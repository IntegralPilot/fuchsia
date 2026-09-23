// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![no_std]

pub use paste::paste;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct LkInitLevel(pub u32);

pub type LkInitHook = extern "C" fn(level: LkInitLevel);

pub const LK_INIT_LEVEL_EARLIEST: LkInitLevel = LkInitLevel(1);

// Arch and platform specific init required to get system into a known state
// and parsing the kernel command line.
//
// Most code should be deferred to later stages if possible, after the command
// line is parsed and a debug UART is available.
pub const LK_INIT_LEVEL_ARCH_EARLY: LkInitLevel = LkInitLevel(0x10000);
pub const LK_INIT_LEVEL_PLATFORM_EARLY: LkInitLevel = LkInitLevel(0x20000);

// Arch and platform specific code that needs to run prior to heap/virtual
// memory being set up.
//
// The kernel command line and a UART is available, but no heap or VM.
pub const LK_INIT_LEVEL_ARCH_PREVM: LkInitLevel = LkInitLevel(0x30000);
pub const LK_INIT_LEVEL_PLATFORM_PREVM: LkInitLevel = LkInitLevel(0x40000);

// Heap and VM initialization.
pub const LK_INIT_LEVEL_VM_PREHEAP: LkInitLevel = LkInitLevel(0x50000);
pub const LK_INIT_LEVEL_HEAP: LkInitLevel = LkInitLevel(0x60000);
pub const LK_INIT_LEVEL_VM: LkInitLevel = LkInitLevel(0x70000);

// Interrupt controller is available.
pub const LK_INIT_LEVEL_INTC: LkInitLevel = LkInitLevel(0x78000);

// Kernel and threading setup.
pub const LK_INIT_LEVEL_TOPOLOGY: LkInitLevel = LkInitLevel(0x80000);
pub const LK_INIT_LEVEL_KERNEL: LkInitLevel = LkInitLevel(0x90000);
pub const LK_INIT_LEVEL_THREADING: LkInitLevel = LkInitLevel(0xa0000);

// Arch and platform specific set up.
//
// Kernel heap, VM, and threads are available. Most init code should go
// in these stages.
pub const LK_INIT_LEVEL_ARCH: LkInitLevel = LkInitLevel(0xb0000);
pub const LK_INIT_LEVEL_PLATFORM: LkInitLevel = LkInitLevel(0xc0000);
pub const LK_INIT_LEVEL_ARCH_LATE: LkInitLevel = LkInitLevel(0xd0000);

// At this level we wait for secondary CPUs to finish booting and "check-in" as ready.
//
// See also mp_wait_for_all_cpus_ready.
pub const LK_INIT_LEVEL_SMP_WAIT: LkInitLevel = LkInitLevel(0xd4000);

// At this level the secondary CPUs have checked-in.
pub const LK_INIT_LEVEL_SMP_READY: LkInitLevel = LkInitLevel(0xd8000);

// Userspace started.
pub const LK_INIT_LEVEL_USER: LkInitLevel = LkInitLevel(0xe0000);
pub const LK_INIT_LEVEL_LAST: LkInitLevel = LkInitLevel(u32::MAX);

#[repr(u32)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum LkInitFlags {
    PrimaryCpu = 0x1,
    SecondaryCpus = 0x2,
    AllCpus = 0x3,
}

#[repr(C)]
pub struct LkInitStruct {
    pub level: LkInitLevel,
    pub flags: LkInitFlags,
    pub hook: LkInitHook,
    pub name: *const core::ffi::c_char,
}

// Safety: LkInitStruct structures placed in the lk_init section are read-only
// after boot and safe to share between threads.
unsafe impl core::marker::Sync for LkInitStruct {}

// Verify size and alignment compatibility with C++ structs.
zr::static_assert!(core::mem::size_of::<LkInitStruct>() == 24);
zr::static_assert!(core::mem::align_of::<LkInitStruct>() == 8);

#[macro_export]
macro_rules! lk_init_hook_flags {
    ($var_name:ident, $hook:expr, $level:expr, $flags:expr) => {
        $crate::paste! {
        extern "C" fn [< _init_struct_fn_wrapper_ $var_name >](level: $crate::LkInitLevel) {
            ($hook)(level)
        }
        #[used]
        #[unsafe(link_section = ".data.rel.ro.lk_init")]
        pub static [<  _init_struct_ $var_name >]: $crate::LkInitStruct = $crate::LkInitStruct {
            level: $level,
            flags: $flags,
            hook: [< _init_struct_fn_wrapper_ $var_name >],
            name: core::concat!(core::stringify!($var_name), "\0").as_ptr()
                as *const core::ffi::c_char,
        };
        }
    };
}

#[macro_export]
macro_rules! lk_init_hook {
    ($var_name:ident, $hook:expr, $level:expr) => {
        $crate::lk_init_hook_flags!($var_name, $hook, $level, $crate::LkInitFlags::PrimaryCpu);
    };
}
