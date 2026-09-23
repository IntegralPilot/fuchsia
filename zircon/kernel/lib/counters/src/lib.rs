// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::cmp::{max, min};
use core::mem::{offset_of, size_of_val};
use core::ptr;
use core::sync::atomic::{AtomicI64, Ordering};

use crate::kernel::percpu::PerCpu;
use crate::platform_rs::timer::{DurationMono, current_mono_time};
use counters_bindings as bindings;

/// The maximum number of CPUs that this counter descriptor supports.
/// This value is read from the `SMP_MAX_CPUS` environment variable at build time.
pub const SMP_MAX_CPUS: usize =
    zr::parse_usize(env!("SMP_MAX_CPUS")).expect("SMP_MAX_CPUS invalid");

pub use zr::to_array;

/// The aggregation type of a kernel counter.
///
/// This specifies how the diagnostic tools should combine the per-CPU slot values
/// of the counter to produce a single diagnostic value.
#[repr(u64)]
pub enum Type {
    /// Padding element (unused).
    Padding = 0,
    /// Standard summation counter (aggregates the sum across all CPUs).
    Sum = 1,
    /// Minimum tracker counter (finds the minimum value across all CPUs).
    Min = 2,
    /// Maximum tracker counter (finds the maximum value across all CPUs).
    Max = 3,
}

/// Binary-stable C-compatible representation of a kernel counter descriptor.
///
/// The memory layout of this structure matches Zircon's `counters::Descriptor` exactly,
/// enabling the linker and userspace diagnostic tools to parse Rust-declared counters
/// seamlessly from the kernel's binary segments.
#[repr(C, align(8))]
pub struct Descriptor {
    name: [u8; 56],
    type_: u64,
}

zr::static_assert!(
    core::mem::size_of::<Descriptor>() == core::mem::size_of::<bindings::counters_Descriptor>()
);
zr::static_assert!(
    core::mem::align_of::<Descriptor>() == core::mem::align_of::<bindings::counters_Descriptor>()
);
zr::static_assert!(
    core::mem::offset_of!(Descriptor, name)
        == core::mem::offset_of!(bindings::counters_Descriptor, name)
);
zr::static_assert!(
    core::mem::offset_of!(Descriptor, type_)
        == core::mem::offset_of!(bindings::counters_Descriptor, type_)
);
zr::static_assert!(Type::Padding as u64 == bindings::counters_Type_kPadding);
zr::static_assert!(Type::Sum as u64 == bindings::counters_Type_kSum);
zr::static_assert!(Type::Min as u64 == bindings::counters_Type_kMin);
zr::static_assert!(Type::Max as u64 == bindings::counters_Type_kMax);

impl Descriptor {
    /// Create a new raw `Descriptor` instance with the given packed name and type value.
    pub const fn new(name: [u8; 56], type_: u64) -> Self {
        Self { name, type_ }
    }
}

// Via magic in kernel.ld, all the descriptors wind up in a contiguous
// array bounded by these two symbols, sorted by name.
unsafe extern "C" {
    static kcountdesc_begin: Descriptor;
    static kcountdesc_end: Descriptor;
}

/// Diagnostic descriptor table metadata.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct CounterDesc;

impl CounterDesc {
    pub const fn new() -> Self {
        Self
    }

    pub fn begin(&self) -> *const Descriptor {
        ptr::addr_of!(kcountdesc_begin)
    }

    pub fn end(&self) -> *const Descriptor {
        ptr::addr_of!(kcountdesc_end)
    }

    pub fn size(&self) -> usize {
        let begin = self.begin() as usize;
        let end = self.end() as usize;
        (end - begin) / size_of::<Descriptor>()
    }
}

/// A thread-safe diagnostic handle representing a self-declared kernel counter.
///
/// This structure contains a pointer to the counter's static `Descriptor` layout in memory,
/// and provides methods to directly query and manipulate the counter across per-CPU slots.
pub struct Counter {
    descriptor: *const Descriptor,
}

unsafe impl Sync for Counter {}
unsafe impl Send for Counter {}

impl Counter {
    /// Create a new Counter handle using the direct descriptor pointer address.
    ///
    /// # Safety
    /// This should only be called with a pointer to a valid, linker-defined
    /// static descriptor variable.
    pub const unsafe fn new_with_ptr(descriptor: *const Descriptor) -> Self {
        Self { descriptor }
    }

    #[inline]
    fn index(&self) -> usize {
        let desc_addr = self.descriptor as usize;
        let begin_addr = CounterDesc::new().begin() as usize;
        (desc_addr - begin_addr) / size_of::<Descriptor>()
    }

    #[inline]
    fn slot_for_cpu<'a>(&self, p: &'a PerCpu) -> &'a AtomicI64 {
        // SAFETY: `p.counters` points to this CPU's slice of the counters arena,
        // which contains an `int64_t` entry for each counter descriptor indexed by `index()`.
        unsafe { &*p.counters.add(self.index()).cast::<AtomicI64>() }
    }

    #[inline]
    fn slot(&self) -> &AtomicI64 {
        self.slot_for_cpu(PerCpu::get_current())
    }

    /// Return the sum of the per-cpu slots for this counter across all CPUs.
    #[inline]
    pub fn sum_across_all_cpus(&self) -> i64 {
        let mut sum: i64 = 0;
        PerCpu::for_each(|_cpu_num, p| {
            sum = sum.wrapping_add(self.slot_for_cpu(p).load(Ordering::Relaxed));
        });
        sum
    }

    /// Return the max of the per-cpu slots for this counter.
    #[inline]
    pub fn max_across_all_cpus(&self) -> i64 {
        let mut max_value = i64::MIN;
        PerCpu::for_each(|_cpu_num, p| {
            max_value = max(max_value, self.slot_for_cpu(p).load(Ordering::Relaxed));
        });
        max_value
    }

    /// Return the min of the per-cpu slots for this counter.
    #[inline]
    pub fn min_across_all_cpus(&self) -> i64 {
        let mut min_value = i64::MAX;
        PerCpu::for_each(|_cpu_num, p| {
            min_value = min(min_value, self.slot_for_cpu(p).load(Ordering::Relaxed));
        });
        min_value
    }

    /// Return the value of the calling cpu's slot for this counter.
    #[inline]
    pub fn value_curr_cpu(&self) -> i64 {
        self.slot().load(Ordering::Relaxed)
    }

    /// Set the value of calling cpu's slot to `value`. No memory order is implied.
    #[inline]
    pub fn set(&self, value: u64) {
        self.slot().store(value as i64, Ordering::Relaxed);
    }

    /// Add the given delta value to the calling CPU's counter slot.
    #[inline]
    pub fn add(&self, delta: i64) {
        let slot = self.slot();
        slot.store(slot.load(Ordering::Relaxed).wrapping_add(delta), Ordering::Relaxed);
    }

    /// Update the calling CPU's counter slot to the minimum of its current value and the given
    /// value.
    #[inline]
    pub fn min(&self, value: i64) {
        let slot = self.slot();
        let current = slot.load(Ordering::Relaxed);
        if value < current {
            slot.store(value, Ordering::Relaxed);
        }
    }

    /// Update the calling CPU's counter slot to the maximum of its current value and the given
    /// value.
    #[inline]
    pub fn max(&self, value: i64) {
        let slot = self.slot();
        let current = slot.load(Ordering::Relaxed);
        if value > current {
            slot.store(value, Ordering::Relaxed);
        }
    }
}

/// Macro to safely define a new Counter in Rust that is visible to the kernel.
///
/// # Example
/// ```rust
/// define_kcounter!(MY_COUNTER, "my.custom.counter", Sum);
///
/// fn some_kernel_code() {
///     MY_COUNTER.add(1);
/// }
/// ```
#[macro_export]
macro_rules! define_kcounter {
    ($rust_var:ident, $name:expr, $type:ident) => {
        pub static $rust_var: $crate::counters::Counter = {
            #[unsafe(link_section = concat!(".bss.kcounter.", $name))]
            #[used]
            static mut ARENA: [i64; $crate::counters::SMP_MAX_CPUS] =
                [0; $crate::counters::SMP_MAX_CPUS];

            #[unsafe(link_section = concat!("kcountdesc.", $name))]
            #[used]
            static DESC: $crate::counters::Descriptor = $crate::counters::Descriptor::new(
                $crate::counters::to_array::<56>($name),
                $crate::counters::Type::$type as u64,
            );

            unsafe {
                $crate::counters::Counter::new_with_ptr(
                    &DESC as *const $crate::counters::Descriptor,
                )
            }
        };
    };
}
pub use define_kcounter;

/// kernel.ld uses this and fills in the descriptor table size after it and then
/// places the sorted descriptor table after that (and then pads to page size),
/// so as to fully populate the counters::DescriptorVmo layout.
#[unsafe(link_section = ".kcounter.desc.header")]
#[used]
static VMO_HEADER: [u64; 2] = [bindings::counters_DescriptorVmo_kMagic, SMP_MAX_CPUS as u64];

zr::static_assert!(
    size_of_val(&VMO_HEADER) == offset_of!(bindings::counters_DescriptorVmo, descriptor_table_size)
);

// This counter tracks how long it takes for Zircon to reach the last init level
// It also can show if the target does not reset the internal clock upon reboot
// which is true also for mexec (netboot) scenario.
define_kcounter!(INIT_TIME, "init.target.time.msec", Sum);

fn counters_init(_level: init::LkInitLevel) {
    INIT_TIME.add(DurationMono::from_nanos(current_mono_time().0).into_millis());
}

init::lk_init_hook!(kcounters, counters_init, init::LkInitLevel(init::LK_INIT_LEVEL_USER.0 - 1));
