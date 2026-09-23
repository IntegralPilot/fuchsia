// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx;

unsafe extern "C" {
    fn __get_unsafe_stack_ptr() -> usize;
}

/// Compute the safe stack pointer inside this function.
///
/// This must never be inlined to ensure the stack pointer is not the one for the previous
/// function.
#[inline(never)]
fn get_safe_stack_ptr() -> Option<usize> {
    let mut sp: usize;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        std::arch::asm!(
            "mov {0}, rsp",
            out(reg) sp,
            options(nomem)
        );
        #[cfg(target_arch = "aarch64")]
        std::arch::asm!(
            "mov {0}, sp",
            out(reg) sp,
            options(nomem)
        );
        #[cfg(target_arch = "riscv64")]
        std::arch::asm!(
            "mv {0}, sp",
            out(reg) sp,
            options(nomem)
        );
    }
    Some(sp)
}

/// Compute the safe stack pointer inside this function.
///
/// This can be inlined because it calls into C, so the underlying call will never be inlined.
fn get_unsafe_stack_ptr() -> Option<usize> {
    // Only x86_64 uses the unsafe stack. Other architecture use the shadow call stack that
    // doesn't need to be cleaned up.
    if cfg!(target_arch = "x86_64") { unsafe { Some(__get_unsafe_stack_ptr()) } } else { None }
}

pub fn clean_stack() {
    // Stacks are 2Mo, clean 1Mo.
    const CLEAN_SIZE: usize = 0x100000;
    let page_size: usize = zx::system_get_page_size() as usize;
    let stack_ptr_factories = [get_unsafe_stack_ptr, get_safe_stack_ptr];
    for factory in stack_ptr_factories {
        // Compute the pointers to the stack as a function call from this function to ensure the
        // pointed memory is unused.
        let Some(v) = factory() else {
            continue;
        };
        // Afford some space on the stack for the op_range call.
        let v = v.checked_sub(64).unwrap_or_else(|| {
            panic!("Starnix kernel stack pointer underflow: v = {v}");
        });
        let max_addr = v - (v % page_size);
        let start_addr = max_addr.checked_sub(CLEAN_SIZE).unwrap_or_else(|| {
            panic!(
                "Starnix kernel stack range calculation underflow: max_addr = {max_addr}, CLEAN_SIZE = {CLEAN_SIZE}"
            );
        });
        let _ =
            fuchsia_runtime::vmar_root_self().op_range(zx::VmarOp::ZERO, start_addr, CLEAN_SIZE);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[inline(never)]
    fn clean_stack_at_level(level: usize) -> usize {
        if level == 0 {
            clean_stack();
            0
        } else {
            1 + clean_stack_at_level(level - 1)
        }
    }

    #[::fuchsia::test]
    fn test_clean_stack() {
        let mut array = [0u8; 256];
        for i in 0..256 {
            array[i] = 255 - (i as u8);
        }
        for i in 0..4096 {
            assert_eq!(clean_stack_at_level(i), i);
        }
        for i in 0..256 {
            assert_eq!(array[i], 255 - (i as u8));
        }
    }
}
