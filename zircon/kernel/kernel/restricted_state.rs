// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::arch_rs::ArchSavedNormalState;
use crate::vm::arch_vm_aspace::{ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE};
use crate::vm::pmm::{ALLOC_FLAG_ANY, ALLOC_FLAG_CAN_WAIT};
use crate::vm::vm_aspace::VmAspace;
use crate::vm::vm_mapping::VmMapping;
use crate::vm::vm_object::VmObject;
use crate::vm::vm_object_paged::VmObjectPaged;
use core::ptr::{self, NonNull};
use debug::ltracef;
use fbl::RefPtr;
use kalloc::Box;
use zx_status::Status;
use zx_types::{zx_exception_report_t, zx_restricted_state_t};

const LOCAL_TRACE: u32 = 0;

const STATE_VMO_SIZE: usize = page::SIZE;

/// Encapsulates a thread's restricted mode state, including VMO backing and mapping.
///
/// The definition of `RestrictedState` lives completely on the Rust side. Instances are allocated
/// in Rust via `RestrictedState::create`, destroyed via `Drop` / `rust_restricted_state_destroy`,
/// and referenced from C++ as an opaque handle pointer.
pub struct RestrictedState {
    in_restricted: bool,
    vector_ptr: usize,
    context: usize,
    exception_report_ptr: Option<NonNull<zx_exception_report_t>>,
    vmo: RefPtr<VmObjectPaged>,
    mapping: RefPtr<VmMapping>,
    state_mapping_ptr: NonNull<zx_restricted_state_t>,
    arch: ArchSavedNormalState,
}

type VmoMapping = (RefPtr<VmObjectPaged>, RefPtr<VmMapping>, NonNull<zx_restricted_state_t>);

/// Allocate a 1-page VMO, commit and pin it, map it into the kernel address space, and eagerly fault in the pages.
fn create_vmo_mapping() -> Result<VmoMapping, Status> {
    // 1. Create a 1-page paged VMO to back the thread's restricted mode state.
    let vmo =
        VmObjectPaged::create(ALLOC_FLAG_ANY | ALLOC_FLAG_CAN_WAIT, 0, STATE_VMO_SIZE as u64)?;

    // 2. Commit and pin the VMO so physical pages are permanently backed.
    vmo.commit_range_pinned(0, STATE_VMO_SIZE as u64, true)?;

    // 3. Create a mapping of this VMO in the kernel address space (`kernel_aspace`).
    // The kernel address space and its root VMAR are kernel singletons initialized at early boot.
    let kernel_vmar =
        VmAspace::kernel_aspace().root_vmar().expect("kernel aspace root VMAR must be initialized");

    // SAFETY: VmObjectPaged inherits from VmObject, so casting its RefPtr to RefPtr<VmObject> is sound.
    let base_vmo = unsafe { vmo.clone().cast::<VmObject>() };
    let mapping_name = c"restricted state";
    let arch_mmu_flags = ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE;

    let map_result = kernel_vmar
        .create_vm_mapping(0, STATE_VMO_SIZE, 0, 0, base_vmo, 0, arch_mmu_flags, mapping_name)
        .inspect_err(|_| {
            vmo.unpin(0, STATE_VMO_SIZE as u64);
        })?;

    // 4. Eagerly fault in all pages so kernel mode never demand-faults on access.
    if let Err(err) = map_result.mapping.map_range(0, STATE_VMO_SIZE, true, false) {
        let _ = map_result.mapping.destroy();
        vmo.unpin(0, STATE_VMO_SIZE as u64);
        return Err(err);
    }

    let state_mapping_ptr = match NonNull::new(core::ptr::with_exposed_provenance_mut::<
        zx_restricted_state_t,
    >(map_result.base))
    {
        Some(ptr) => ptr,
        None => {
            let _ = map_result.mapping.destroy();
            vmo.unpin(0, STATE_VMO_SIZE as u64);
            return Err(Status::NO_MEMORY);
        }
    };

    Ok((vmo, map_result.mapping, state_mapping_ptr))
}

impl RestrictedState {
    /// Create a new `RestrictedState`, allocating its VMO and mapping in kernel memory.
    pub fn create(
        exception_report_ptr: Option<NonNull<zx_exception_report_t>>,
    ) -> Result<Box<Self>, Status> {
        let (vmo, mapping, state_mapping_ptr) = create_vmo_mapping()?;

        ltracef!("mapping at {:#x}\n", state_mapping_ptr.as_ptr() as usize);

        Box::try_new(Self {
            in_restricted: false,
            vector_ptr: 0,
            context: 0,
            exception_report_ptr,
            vmo,
            mapping,
            state_mapping_ptr,
            arch: ArchSavedNormalState::default(),
        })
        .map_err(|_| Status::NO_MEMORY)
    }

    /// Create a new `RestrictedState` from a raw `zx_exception_report_t` pointer.
    pub fn create_from_raw(
        exception_report_ptr: *mut zx_exception_report_t,
    ) -> Result<Box<Self>, Status> {
        Self::create(NonNull::new(exception_report_ptr))
    }

    /// Return whether the thread is currently in restricted mode.
    pub fn in_restricted(&self) -> bool {
        self.in_restricted
    }

    /// Set whether the thread is currently in restricted mode.
    pub fn set_in_restricted(&mut self, val: bool) {
        self.in_restricted = val;
    }

    /// Return the normal mode vector table pointer.
    pub fn vector_ptr(&self) -> usize {
        self.vector_ptr
    }

    /// Set the normal mode vector table pointer.
    pub fn set_vector_ptr(&mut self, val: usize) {
        self.vector_ptr = val;
    }

    /// Return the normal mode context value.
    pub fn context(&self) -> usize {
        self.context
    }

    /// Set the normal mode context value.
    pub fn set_context(&mut self, val: usize) {
        self.context = val;
    }

    /// Return the user exception report pointer as an `Option<NonNull<zx_exception_report_t>>`.
    pub fn exception_report_ptr(&self) -> Option<NonNull<zx_exception_report_t>> {
        self.exception_report_ptr
    }

    /// Return the user exception report pointer as a raw pointer.
    pub fn exception_report_raw_ptr(&self) -> *mut zx_exception_report_t {
        self.exception_report_ptr.map_or(ptr::null_mut(), |p| p.as_ptr())
    }

    /// Return a shared reference to the saved normal mode architecture state.
    pub fn arch_normal_state(&self) -> &ArchSavedNormalState {
        &self.arch
    }

    /// Return a mutable reference to the saved normal mode architecture state.
    pub fn arch_normal_state_mut(&mut self) -> &mut ArchSavedNormalState {
        &mut self.arch
    }

    /// Return a shared reference to the mapped `zx_restricted_state_t` buffer.
    pub fn state(&self) -> &zx_restricted_state_t {
        // SAFETY: By RestrictedState invariant, state_mapping_ptr points to a valid zx_restricted_state_t.
        unsafe { self.state_mapping_ptr.as_ref() }
    }

    /// Return a mutable reference to the mapped `zx_restricted_state_t` buffer.
    pub fn state_mut(&mut self) -> &mut zx_restricted_state_t {
        // SAFETY: By RestrictedState invariant, state_mapping_ptr points to a valid zx_restricted_state_t.
        unsafe { self.state_mapping_ptr.as_mut() }
    }

    /// Return a raw pointer to the mapped `zx_restricted_state_t` buffer.
    pub fn state_ptr(&self) -> *mut zx_restricted_state_t {
        self.state_mapping_ptr.as_ptr()
    }

    /// Return a raw pointer to the mapped state buffer cast to `*mut T`.
    pub fn state_ptr_as<T>(&self) -> *mut T {
        self.state_mapping_ptr.as_ptr().cast::<T>()
    }

    /// Returns a shared reference to the VMO backing the shared mapping.
    pub fn vmo(&self) -> &VmObject {
        &self.vmo
    }
}

impl Drop for RestrictedState {
    fn drop(&mut self) {
        // Destroy the kernel mapping and unpin the VMO pages.
        let _ = self.mapping.destroy();
        self.vmo.unpin(0, STATE_VMO_SIZE as u64);
    }
}

/// Destroy a `RestrictedState` created by `RestrictedState::create`.
///
/// # Safety
///
/// Caller must pass a pointer returned by `RestrictedState::create` exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_restricted_state_destroy(ptr: *mut RestrictedState) {
    if !ptr.is_null() {
        // SAFETY: ptr was created by Box::into_raw in RestrictedState::create and passed once.
        unsafe {
            let _ = Box::from_raw(ptr);
        }
    }
}

/// Return whether the restricted state indicates that the thread is in restricted mode.
///
/// # Safety
///
/// `state` must be a valid, non-null reference to a `RestrictedState`.
#[unsafe(no_mangle)]
pub extern "C" fn rust_restricted_state_in_restricted(state: &RestrictedState) -> bool {
    state.in_restricted()
}

/// Restricted state kernel unit tests.
#[cfg(ktest)]
#[unittest::suite(name = "restricted_state_tests")]
mod tests {
    use super::{
        NonNull, RestrictedState, rust_restricted_state_destroy,
        rust_restricted_state_in_restricted, zx_exception_report_t, zx_restricted_state_t,
    };

    /// Verifies creation of RestrictedState and basic field accessors and mutators.
    #[test]
    fn test_restricted_state_creation_and_accessors() {
        let mut rs = RestrictedState::create(None).expect("failed to create RestrictedState");
        unittest::expect_false!(rs.in_restricted());
        rs.set_in_restricted(true);
        unittest::expect_true!(rs.in_restricted());
        rs.set_in_restricted(false);
        unittest::expect_false!(rs.in_restricted());

        unittest::expect_true!(rs.vector_ptr() == 0);
        rs.set_vector_ptr(0x12345678);
        unittest::expect_true!(rs.vector_ptr() == 0x12345678);

        unittest::expect_true!(rs.context() == 0);
        rs.set_context(0x9abcdef0);
        unittest::expect_true!(rs.context() == 0x9abcdef0);

        unittest::expect_true!(rs.exception_report_ptr().is_none());
        unittest::expect_true!(rs.exception_report_raw_ptr().is_null());

        unittest::expect_false!(rs.state_ptr().is_null());
        unittest::expect_true!(rs.state_ptr_as::<zx_restricted_state_t>() == rs.state_ptr());
    }

    /// Verifies creation of RestrictedState when provided with an exception report pointer.
    #[test]
    fn test_restricted_state_with_exception_report() {
        // SAFETY: zx_exception_report_t contains primitive field types and zero-initialization is sound.
        let mut report = unsafe { core::mem::zeroed::<zx_exception_report_t>() };
        let report_ptr = NonNull::new(&mut report as *mut zx_exception_report_t).unwrap();
        let rs = RestrictedState::create(Some(report_ptr))
            .expect("failed to create RestrictedState with exception report");
        unittest::expect_true!(rs.exception_report_ptr() == Some(report_ptr));
        unittest::expect_true!(rs.exception_report_raw_ptr() == report_ptr.as_ptr());
    }

    /// Test FFI lifecycle routines (destroy, in_restricted).
    #[test]
    fn test_restricted_state_ffi_lifecycle() {
        let raw_ptr = Box::into_raw(RestrictedState::create(None).unwrap());
        unittest::expect_false!(raw_ptr.is_null());

        // SAFETY: raw_ptr points to a valid, initialized RestrictedState returned by create.
        unsafe {
            unittest::expect_false!(rust_restricted_state_in_restricted(&*raw_ptr));
        }

        // SAFETY: raw_ptr was returned by rust_restricted_state_create and has not been destroyed.
        unsafe {
            rust_restricted_state_destroy(raw_ptr);
        }
    }
}
