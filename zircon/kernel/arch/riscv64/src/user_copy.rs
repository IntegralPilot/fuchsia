// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Fault-tolerant memory copying between user and kernel spaces for RISC-V 64.

use crate::arch_rs::{FaultInfo, UserCopyCaptureFaultsError};
use zx_status::Status;

/// Bit in `data_fault_resume` to request fault capturing instead of immediate panic/trap.
pub use riscv64_user_copy_bindings::RISCV_CAPTURE_USER_COPY_FAULTS_BIT;

/// RISC-V canonical user address mask (Sv39/Sv48 top bit mask).
/// Return value representation from `_riscv64_user_copy`.
type Riscv64UserCopyRet = riscv64_user_copy_bindings::Riscv64UserCopyRet;

const _: () = {
    assert!(core::mem::size_of::<Riscv64UserCopyRet>() == 16);
    assert!(core::mem::align_of::<Riscv64UserCopyRet>() == 8);
};

unsafe extern "C" {
    fn _riscv64_user_copy(
        dst: *mut core::ffi::c_void,
        src: *const core::ffi::c_void,
        len: usize,
        fault_return: *mut u64,
        capture_faults_mask: u64,
    ) -> Riscv64UserCopyRet;
}

#[inline(always)]
fn current_thread_data_fault_resume_ptr() -> *mut u64 {
    let thread = crate::kernel::thread::current_get();
    // SAFETY: `current_get()` returns this CPU's running thread, and `thread_arch`
    // only offsets the pointer; taking a field address reads nothing.
    unsafe {
        let arch = super::thread::thread_arch(thread.cast());
        core::ptr::addr_of_mut!((*arch).data_fault_resume)
    }
}

/// Canonical address mask for RISC-V 64 Sv39 virtual addresses; a set bit above
/// bit 37 means the address is outside the user half of the address space.
///
/// [riscv/priv/v1.12]: Section 4.4.1 (Sv39: Page-Based 39-bit Virtual-Memory System)
const RISCV64_CANONICAL_ADDRESS_MASK: usize = !((1usize << 38) - 1);

/// Check if a virtual address is in the user-accessible address space.
#[inline(always)]
pub fn is_user_accessible(va: usize) -> bool {
    (va & RISCV64_CANONICAL_ADDRESS_MASK) == 0
}

/// Check that the continuous range of addresses in `[va, va + len)` are all user-accessible.
fn is_user_accessible_range(va: usize, len: usize) -> bool {
    let Some(end) = va.checked_add(len) else {
        return false;
    };

    is_user_accessible(va) && (len == 0 || is_user_accessible(end - 1))
}

/// Copy memory from user space (`src`) to kernel space (`dst`).
///
/// # Safety
/// `dst` must be valid for writes of `len` bytes. `src` is user-supplied and is
/// only required to be a user-half address: a fault while reading it is caught
/// and reported as an error rather than trapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_copy_from_user(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    len: usize,
) -> Result<(), Status> {
    if !is_user_accessible_range(src as usize, len) {
        return Err(Status::INVALID_ARGS);
    }
    let fault_resume_ptr = current_thread_data_fault_resume_ptr();
    // SAFETY: the range was just checked to lie entirely in the user half of the
    // address space, and `fault_resume_ptr` points at the current thread's
    // `data_fault_resume` field, which `_riscv64_user_copy` uses to install and
    // restore its fault handler. A fault on the user side unwinds through that
    // handler rather than trapping.
    let ret = unsafe { _riscv64_user_copy(dst, src, len, fault_resume_ptr, 0) };
    Status::ok(ret.status)
}

/// Copy memory from kernel space (`src`) to user space (`dst`).
///
/// # Safety
/// `src` must be valid for reads of `len` bytes. `dst` is user-supplied and is
/// only required to be a user-half address: a fault while writing it is caught
/// and reported as an error rather than trapping.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_copy_to_user(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    len: usize,
) -> Result<(), Status> {
    if !is_user_accessible_range(dst as usize, len) {
        return Err(Status::INVALID_ARGS);
    }
    let fault_resume_ptr = current_thread_data_fault_resume_ptr();
    // SAFETY: the range was just checked to lie entirely in the user half of the
    // address space, and `fault_resume_ptr` points at the current thread's
    // `data_fault_resume` field, which `_riscv64_user_copy` uses to install and
    // restore its fault handler. A fault on the user side unwinds through that
    // handler rather than trapping.
    let ret = unsafe { _riscv64_user_copy(dst, src, len, fault_resume_ptr, 0) };
    Status::ok(ret.status)
}

/// Copies `len` bytes between user and kernel space, capturing any page faults.
pub fn user_copy_capture_faults(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    len: usize,
    check_addr: usize,
) -> Result<(), UserCopyCaptureFaultsError> {
    if !is_user_accessible_range(check_addr, len) {
        return Err(UserCopyCaptureFaultsError { status: Status::INVALID_ARGS, fault_info: None });
    }
    let fault_resume_ptr = current_thread_data_fault_resume_ptr();
    // SAFETY: the range was just checked to lie entirely in the user half of the
    // address space, and `fault_resume_ptr` points at the current thread's
    // `data_fault_resume` field, which `_riscv64_user_copy` uses to install and
    // restore its fault handler. A fault on the user side unwinds through that
    // handler rather than trapping.
    let ret = unsafe {
        _riscv64_user_copy(
            dst,
            src,
            len,
            fault_resume_ptr,
            RISCV_CAPTURE_USER_COPY_FAULTS_BIT as u64,
        )
    };
    if let Err(status) = Status::ok(ret.status) {
        let fault_info = if ret.pf_va != 0 || ret.pf_flags != 0 {
            Some(FaultInfo { pf_va: ret.pf_va as usize, pf_flags: ret.pf_flags })
        } else {
            None
        };
        Err(UserCopyCaptureFaultsError { status, fault_info })
    } else {
        Ok(())
    }
}

/// Copies `len` bytes from user memory at `src` into kernel memory at `dst`, capturing any page
/// faults.
///
/// # Safety
/// Caller must ensure `dst` points to at least `len` bytes of valid memory
/// and `src` is a user pointer.
pub unsafe fn arch_copy_from_user_capture_faults(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    len: usize,
) -> Result<(), UserCopyCaptureFaultsError> {
    user_copy_capture_faults(dst, src, len, src as usize)
}

/// Copies `len` bytes from kernel memory at `src` into user memory at `dst`, capturing any page
/// faults.
///
/// # Safety
/// Caller must ensure `src` points to at least `len` bytes of valid memory
/// and `dst` is a user pointer.
pub unsafe fn arch_copy_to_user_capture_faults(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    len: usize,
) -> Result<(), UserCopyCaptureFaultsError> {
    user_copy_capture_faults(dst, src, len, dst as usize)
}

// C FFI exports

/// # Safety
/// Caller guarantees valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_copy_from_user_capture_faults(
    dst: *mut u8,
    src: *const u8,
    len: usize,
) -> Riscv64UserCopyRet {
    // SAFETY: Foreign function caller guarantees valid pointers.
    match unsafe { arch_copy_from_user_capture_faults(dst.cast(), src.cast(), len) } {
        Ok(()) => {
            Riscv64UserCopyRet { status: Status::result_into_raw(Ok(())), pf_flags: 0, pf_va: 0 }
        }
        Err(UserCopyCaptureFaultsError { status: _, fault_info: None }) => {
            Riscv64UserCopyRet { status: Status::OUT_OF_RANGE.into_raw(), pf_flags: 0, pf_va: 0 }
        }
        Err(UserCopyCaptureFaultsError { status, fault_info: Some(fault) }) => Riscv64UserCopyRet {
            status: status.into_raw(),
            pf_flags: fault.pf_flags,
            pf_va: fault.pf_va,
        },
    }
}

/// # Safety
/// Caller guarantees valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_copy_to_user_capture_faults(
    dst: *mut u8,
    src: *const u8,
    len: usize,
) -> Riscv64UserCopyRet {
    // SAFETY: Foreign function caller guarantees valid pointers.
    match unsafe { arch_copy_to_user_capture_faults(dst.cast(), src.cast(), len) } {
        Ok(()) => {
            Riscv64UserCopyRet { status: Status::result_into_raw(Ok(())), pf_flags: 0, pf_va: 0 }
        }
        Err(UserCopyCaptureFaultsError { status: _, fault_info: None }) => {
            Riscv64UserCopyRet { status: Status::OUT_OF_RANGE.into_raw(), pf_flags: 0, pf_va: 0 }
        }
        Err(UserCopyCaptureFaultsError { status, fault_info: Some(fault) }) => Riscv64UserCopyRet {
            status: status.into_raw(),
            pf_flags: fault.pf_flags,
            pf_va: fault.pf_va,
        },
    }
}

#[cfg(ktest)]
/// Unit tests for user memory range accessibility checks.
#[unittest::suite(name = "riscv64_user_copy")]
mod tests {
    use super::{is_user_accessible, is_user_accessible_range};
    use unittest::{assert_false, assert_true};

    /// Test address range validation for user memory accesses.
    #[test]
    fn test_user_accessible_range() {
        assert_true!(is_user_accessible(0x1000));
        assert_true!(is_user_accessible(0x0000003f_ffffffff));
        assert_false!(is_user_accessible(0xffffffc0_00000000));

        assert_true!(is_user_accessible_range(0x1000, 0x1000));
        assert_false!(is_user_accessible_range(0x0000003f_ffffffff, 2));
        assert_false!(is_user_accessible_range(usize::MAX - 10, 20));
    }
}
