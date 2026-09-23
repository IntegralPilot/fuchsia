// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_arch = "aarch64")]
mod arm64;

#[cfg(target_arch = "aarch64")]
pub use arm64::*;

#[cfg(target_arch = "x86_64")]
mod x64;

#[cfg(target_arch = "x86_64")]
pub use x64::*;

#[cfg(target_arch = "riscv64")]
mod riscv64;

#[cfg(target_arch = "riscv64")]
pub use riscv64::*;

use starnix_logging::{CATEGORY_STARNIX, NAME_MAP_RESTRICTED_STATE};
use starnix_uapi::__static_assertions::assert_not_impl_any;
use std::ops::Deref;
use std::ptr::NonNull;

/// `RestrictedState` manages accesses into the restricted state VMO.
///
/// See `zx_restricted_bind_state`.
pub struct RestrictedState {
    pub bound_state: NonNull<zx::sys::zx_restricted_state_t>,
    state_size: usize,
}

impl RestrictedState {
    /// Allocates a VMO for the restricted state and maps it into the Starnix kernel.
    ///
    /// The VMO is created using `zx_restricted_bind_state` and then mapped into the
    /// kernel's address space. This allows Starnix to inspect and modify the
    /// restricted state (registers) of the task.
    pub fn bind_and_map(
        register_state: &mut RegisterState<RegisterStorageEnum>,
        exception_report: &mut zx::sys::zx_exception_report_t,
    ) -> Result<Self, zx::Status> {
        fuchsia_trace::duration!(CATEGORY_STARNIX, NAME_MAP_RESTRICTED_STATE);
        let mut out_vmo_handle = 0;
        // SAFETY: `out_vmo_handle` is a valid pointer to a handle on the stack.
        let status = zx::Status::ok(unsafe {
            zx::sys::zx_restricted_bind_state(
                0,
                &mut out_vmo_handle,
                std::ptr::from_mut(exception_report),
            )
        });
        match status {
            Ok(()) => {
                // We've successfully attached the VMO to the current thread. This VMO will be
                // mapped and used for the kernel to store restricted mode register state as it
                // enters and exits restricted mode.
            }
            Err(status) => panic!("zx_restricted_bind_state failed with {status}!"),
        }
        // SAFETY: `out_vmo_handle` is a valid handle as `zx_restricted_bind_state` returned OK.
        let state_vmo = unsafe { zx::Vmo::from(zx::NullableHandle::from_raw(out_vmo_handle)) };

        // Name the VMO so external tools (like the CPU profiler) can capture
        // this specific VMO.
        let name = format!(
            "restricted_state_vmo:{}",
            fuchsia_runtime::with_thread_self(|t| t.koid())?.raw_koid()
        );
        let name = zx::Name::new(&name)?;
        state_vmo.set_name(&name)?;

        let state_size = state_vmo.get_size()? as usize;
        if state_size < std::mem::size_of::<zx::sys::zx_restricted_exception_t>() {
            return Err(zx::Status::INVALID_ARGS);
        }

        let state_address = fuchsia_runtime::vmar_root_self().map(
            0,
            &state_vmo,
            0,
            state_size,
            zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
        )?;

        // This memory is not managed by Rust's stack, heap, etc. so treat it as "foreign" memory
        // with no provenance.
        let state_address: *mut zx::sys::zx_restricted_state_t =
            std::ptr::without_provenance_mut(state_address);
        assert!(state_address.is_aligned(), "Zircon must map restricted-state-aligned memory");
        let bound_state =
            NonNull::new(state_address).expect("Zircon must map non-null restricted-state");

        // Copy the initial register state into the mapped VMO and link the VMO to the register
        // state of the current thread.
        // SAFETY: `bound_state` is valid to read/write to as long as `RestrictedState` is live.
        unsafe {
            let vmo_ptr = bound_state.as_ptr();
            vmo_ptr.write(**register_state);
            register_state.real_registers =
                RegisterStorageEnum::Vmo(MappedVmoRegs(NonNull::new_unchecked(vmo_ptr)));
        }

        Ok(Self { state_size, bound_state })
    }
}

impl std::ops::Drop for RestrictedState {
    fn drop(&mut self) {
        let mapping_addr = self.bound_state.as_ptr() as usize;
        // Safety: We are un-mapping the state VMO. This is safe because we route all access
        // into this memory region though this struct so it is safe to unmap on Drop.
        unsafe {
            fuchsia_runtime::vmar_root_self()
                .unmap(mapping_addr, self.state_size)
                .expect("Failed to unmap");
            zx::sys::zx_restricted_unbind_state(0);
        }
    }
}

pub trait RegisterStorage:
    std::ops::Deref<Target = zx::sys::zx_restricted_state_t>
    + std::ops::DerefMut
    + Eq
    + PartialEq
    + std::fmt::Debug
    + Clone
{
}

#[derive(Eq, PartialEq, Clone)]
struct MappedVmoRegs(NonNull<zx::sys::zx_restricted_state_t>);
// MappedVmoRegs should be tied to the CurrentTask, so it is not Send or Sync.
assert_not_impl_any!(MappedVmoRegs: Send, Sync);

impl std::fmt::Debug for MappedVmoRegs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("MappedVmoRegs").field(&format_args!("{:?}", self.deref())).finish()
    }
}

impl std::ops::Deref for MappedVmoRegs {
    type Target = zx::sys::zx_restricted_state_t;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The pointer is valid and points to a valid `zx_restricted_state_t`.
        unsafe { self.0.as_ref() }
    }
}

impl std::ops::DerefMut for MappedVmoRegs {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: The pointer is valid and points to a valid `zx_restricted_state_t`.
        unsafe { self.0.as_mut() }
    }
}

impl RegisterStorage for MappedVmoRegs {}

#[derive(Eq, PartialEq, Debug, Clone, Default)]
pub struct HeapRegs(Box<zx::sys::zx_restricted_state_t>);

impl std::ops::Deref for HeapRegs {
    type Target = zx::sys::zx_restricted_state_t;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for HeapRegs {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl RegisterStorage for HeapRegs {}

impl From<RegisterStorageEnum> for HeapRegs {
    fn from(regs: RegisterStorageEnum) -> Self {
        match regs {
            RegisterStorageEnum::Vmo(vmo) => HeapRegs(Box::new(*vmo)),
            RegisterStorageEnum::Heap(heap) => heap,
        }
    }
}

/// An enum to hold the registers in either a heap or a vmo.
///
/// This is introduced to allow `CurrentTask` to store registers in heap during initialization and
/// link to the vmo after the `RestrictedState` is created.
#[derive(Eq, PartialEq, Debug)]
pub enum RegisterStorageEnum {
    // Keep it private to prevent from using it directly.
    #[allow(private_interfaces)]
    Vmo(MappedVmoRegs),
    Heap(HeapRegs),
}
assert_not_impl_any!(RegisterStorageEnum: Send, Sync);

impl std::ops::Deref for RegisterStorageEnum {
    type Target = zx::sys::zx_restricted_state_t;

    fn deref(&self) -> &Self::Target {
        match self {
            RegisterStorageEnum::Vmo(vmo) => vmo.deref(),
            RegisterStorageEnum::Heap(heap) => heap.deref(),
        }
    }
}

impl std::ops::DerefMut for RegisterStorageEnum {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            RegisterStorageEnum::Vmo(vmo) => vmo.deref_mut(),
            RegisterStorageEnum::Heap(heap) => heap.deref_mut(),
        }
    }
}

impl Clone for RegisterStorageEnum {
    fn clone(&self) -> RegisterStorageEnum {
        RegisterStorageEnum::Heap(HeapRegs(Box::new(**self)))
    }
}

impl RegisterStorage for RegisterStorageEnum {}

impl From<MappedVmoRegs> for RegisterStorageEnum {
    fn from(regs: MappedVmoRegs) -> Self {
        RegisterStorageEnum::Vmo(regs)
    }
}

impl From<HeapRegs> for RegisterStorageEnum {
    fn from(regs: HeapRegs) -> Self {
        RegisterStorageEnum::Heap(regs)
    }
}
