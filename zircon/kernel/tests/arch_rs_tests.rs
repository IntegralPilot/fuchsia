// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#[unsafe(no_mangle)]
pub extern "C" fn rust_arch_rs_tests_interrupt_ops() -> bool {
    let initially_disabled = crate::arch_rs::ints_disabled();
    if initially_disabled {
        return false;
    }
    crate::arch_rs::disable_ints();
    if !crate::arch_rs::ints_disabled() {
        return false;
    }
    crate::arch_rs::enable_ints();
    !crate::arch_rs::ints_disabled()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_arch_rs_tests_curr_cpu_num() -> u32 {
    crate::arch_rs::curr_cpu_num()
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_arch_rs_tests_max_num_cpus() -> u32 {
    crate::arch_rs::max_num_cpus()
}
