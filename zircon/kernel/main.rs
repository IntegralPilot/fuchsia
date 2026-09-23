// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![no_std]
#![no_main]
// TODO(https://fxbug.dev/539292628): Allow dead code during the initial Rust
// conversion process.
#![allow(dead_code)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::new_without_default)]
#![feature(cfg_sanitize)]

#[allow(unused_imports)]
#[macro_use]
extern crate ktrace_macro;

// Submodules in the kernel crate:

#[path = "arch/src/mod.rs"]
pub mod arch_rs;

#[path = "dev/interrupt/interrupt.rs"]
pub mod dev_interrupt;

#[path = "dev/pdev/hw_watchdog/watchdog.rs"]
pub mod pdev_watchdog;

#[path = "dev/pdev/power/power.rs"]
pub mod pdev_power;

#[path = "kernel/mod.rs"]
pub mod kernel;

#[path = "lib/cbuf/src/mod.rs"]
pub mod cbuf;

#[path = "lib/console/mod.rs"]
pub mod console_rust;

#[path = "lib/counters/src/lib.rs"]
pub mod counters;

#[path = "lib/debuglog/debuglog.rs"]
pub mod debuglog_rs;

#[path = "lib/ktrace/src/mod.rs"]
pub mod ktrace_rs;

#[path = "lib/mmu_rcu/src/mod.rs"]
pub mod mmu_rcu;

#[path = "lib/page_cache/page_cache.rs"]
pub mod page_cache;

#[path = "lib/persistent-debuglog/persistent_debuglog.rs"]
pub mod persistent_debuglog_rs;

#[path = "lib/root_resource_filter/src/mod.rs"]
pub mod root_resource_filter;

#[path = "lib/syscalls/mod.rs"]
pub mod syscalls_rs;

#[path = "lib/topology/src/mod.rs"]
pub mod topology;

#[path = "lib/user_copy/src/mod.rs"]
pub mod user_copy;

#[path = "lib/userabi/vdso.rs"]
pub mod userabi;

#[path = "object/object.rs"]
pub mod object;

#[path = "platform/platform.rs"]
pub mod platform_rs;

#[path = "top/mod.rs"]
pub mod top;

#[path = "top/rust_panic.rs"]
pub mod rust_panic;

#[path = "vm/mod.rs"]
pub mod vm;

// Target/board-specific modules:

#[cfg(target_arch = "x86_64")]
#[path = "platform/pc/mod.rs"]
pub mod platform_pc;

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[path = "dev/pdev/interrupt/interrupt.rs"]
pub mod pdev_interrupt;

#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[path = "dev/hw_watchdog/generic32/generic32.rs"]
pub mod generic32;

#[cfg(target_arch = "riscv64")]
#[path = "dev/interrupt/plic/plic.rs"]
pub mod plic;

#[cfg(target_arch = "aarch64")]
#[path = "dev/power/iris/iris.rs"]
pub mod power_iris;

#[cfg(target_arch = "aarch64")]
#[path = "dev/power/moonflower/moonflower.rs"]
pub mod power_moonflower;

#[cfg(target_arch = "aarch64")]
#[path = "dev/power/motmot/motmot.rs"]
pub mod power_motmot;

// Test modules:

#[cfg(ktest)]
#[path = "tests/arch_rs_tests.rs"]
pub mod arch_rs_tests;

#[cfg(ktest)]
#[path = "lib/console/tests/mod.rs"]
pub mod console_tests_rust;

#[cfg(ktest)]
#[path = "lib/pow2_range_allocator/tests/kernel.rs"]
pub mod pow2_range_allocator_tests;

#[cfg(all(console_enabled, ktest))]
#[path = "lib/unittest/console.rs"]
pub mod unittest_console;

#[cfg(ktest)]
#[path = "lib/unittest/user_memory.rs"]
pub mod user_memory;

#[cfg(ktest)]
#[path = "lib/user_copy/tests/kernel.rs"]
pub mod user_copy_rs_kernel_tests;

#[cfg(ktest)]
#[path = "vm/unittests/mod.rs"]
pub mod vm_unittests;

#[cfg(ktest)]
#[path = "../../src/lib/ksync/tests/kernel.rs"]
pub mod ksync_kernel_tests;

#[cfg(ktest)]
#[path = "tests/rustlang-support/test_mod.rs"]
pub mod test_mod;

#[cfg(ktest)]
#[path = "tests/unused_rust.rs"]
pub mod unused_rust;

// Force linking of extern crates that provide C entry points:
use flow_id as _;
use init as _;
use relaxed_atomic as _;
#[cfg(ktest)]
use trivial_tests as _;
