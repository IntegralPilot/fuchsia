// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Root resource filter implementation in Rust.
//
// Tracks denied resource regions (such as RAM and exclusive peripherals) to ensure
// that userspace access cannot be granted even with the root resource.

use crate::counters::define_kcounter;
use core::mem::MaybeUninit;
use core::ptr;
use debug::dprintf::{SPEW, dprintf_enabled};
use debug::{dprintf, ltracef};
use pin_init::{InPlaceWrite as _, PinInit, pin_data, pin_init};
use region_alloc::{AllowOverlap, RegionAllocator, RegionSpan, TestRegionSet};
use zx_types::{ZX_RSRC_KIND_MMIO, zx_rsrc_kind_t};

#[cfg(target_arch = "x86_64")]
use zx_types::ZX_RSRC_KIND_IOPORT;

const LOCAL_TRACE: u32 = 0;

define_kcounter!(RESOURCE_RANGES_DENIED, "resource.denied_ranges", Sum);

unsafe extern "C" {
    fn cpp_root_resource_filter_populate_finalized_ranges(
        context: *const core::ffi::c_void,
        callback: unsafe extern "C" fn(filter: &RootResourceFilter, base: u64, size: u64),
    );
}

/// The `RootResourceFilter` tracks the regions of the various resource address
/// spaces we may never grant access to, even if the user has access to the root
/// resource. Currently this only affects the MMIO space (and I/O port space on x86_64).
/// Any attempt to register a deny range for some other resource will succeed, but no
/// enforcement will happen. The current set of denied MMIO ranges should consist of:
///
/// 1) All physical RAM. RAM is under the control of the PMM. If a user wants
///    access to RAM, they need to obtain it via VMO allocations, not by
///    requesting a specific region of the physical bus using
///    `zx_vmo_create_physical`.
/// 2) Any other regions the platform code considers to be off limits. This
///    usually means things like the interrupt controller registers, the IOMMU
///    registers, and so on.
///
/// Note that we don't bother assigning a `RegionPool` to our region allocator,
/// instead we permit it to allocate directly from the heap. The set of
/// regions that we need to deny is 100% known to us, but it is never going to
/// be a large number of regions, and once established it will never change.
/// There is no good reason to partition the bookkeeping allocations into their
/// own separate slab allocated pool.
#[repr(C)]
#[pin_data]
pub struct RootResourceFilter {
    #[pin]
    mmio_deny: RegionAllocator,
    #[cfg(target_arch = "x86_64")]
    #[pin]
    ioport_deny: RegionAllocator,
}

static mut GLOBAL_FILTER: MaybeUninit<RootResourceFilter> = MaybeUninit::uninit();

/// Initializes `GLOBAL_FILTER` during early boot before any other subsystem runs.
fn init_root_resource_filter(_level: init::LkInitLevel) {
    // SAFETY: Called once during single-threaded early boot before any other CPUs, threads, or
    // subsystems exist.
    unsafe {
        let filter = &mut *ptr::addr_of_mut!(GLOBAL_FILTER);
        let _ = filter.write_pin_init(RootResourceFilter::init());
    }
}

init::lk_init_hook!(
    init_root_resource_filter,
    init_root_resource_filter,
    init::LK_INIT_LEVEL_EARLIEST
);

impl RootResourceFilter {
    /// Initializes a new `RootResourceFilter` via `PinInit`.
    pub fn init() -> impl PinInit<Self> {
        pin_init!(Self {
            mmio_deny <- RegionAllocator::init(),
            #[cfg(target_arch = "x86_64")]
            ioport_deny <- RegionAllocator::init(),
        })
    }

    /// Returns the global singleton instance of the `RootResourceFilter`.
    pub fn global() -> &'static RootResourceFilter {
        // SAFETY: `GLOBAL_FILTER` is initialized during early boot via `lk_init_hook` at
        // `LK_INIT_LEVEL_EARLIEST`, before any architecture, platform, or driver code runs.
        unsafe { &*ptr::addr_of!(GLOBAL_FILTER).cast::<RootResourceFilter>() }
    }

    /// Adds the range `[base, base + size)` to the range of regions for `kind` to
    /// deny access to. In the event that this range intersects any other
    /// pre-existing ranges, the ranges will be merged as appropriate.
    pub fn add_deny_region(&self, base: usize, size: usize, kind: zx_rsrc_kind_t) {
        // Currently, we only support enforcement of denied MMIO/IOPort ranges depending on the
        // architecture.
        //
        // Any unsupported range `kind` is silently accepted.
        let res = match kind {
            ZX_RSRC_KIND_MMIO => {
                let aligned_base = page::round_down(base);
                let aligned_size = page::round_up((base - aligned_base) + size);
                self.mmio_deny.add_region(
                    RegionSpan { base: aligned_base as u64, size: aligned_size as u64 },
                    AllowOverlap::Yes,
                )
            }
            #[cfg(target_arch = "x86_64")]
            ZX_RSRC_KIND_IOPORT => self
                .ioport_deny
                .add_region(RegionSpan { base: base as u64, size: size as u64 }, AllowOverlap::Yes),
            _ => Ok(()),
        };
        // All deny regions should end up getting added early during kernel
        // startup. Failure to add a region implies heap allocation failure. Not
        // only should it never happen, _not_ enforcing our deny list is not an
        // option. Panic if this happens.
        assert_eq!(res, Ok(()));
    }

    /// Test to see if the specified region is permitted or not.
    pub fn is_region_allowed(&self, base: usize, size: usize, kind: zx_rsrc_kind_t) -> bool {
        let allocator = match kind {
            ZX_RSRC_KIND_MMIO => &self.mmio_deny,
            #[cfg(target_arch = "x86_64")]
            ZX_RSRC_KIND_IOPORT => &self.ioport_deny,
            _ => return true,
        };
        !allocator
            .test_region_intersects(
                RegionSpan { base: base as u64, size: size as u64 },
                TestRegionSet::Available,
            )
            .unwrap_or(true)
    }

    /// Called just before going to user-mode. This will add to the filter all
    /// of the areas known to the PMM at the time, and finally subtract out any
    /// regions present in the ZBI memory config which are flagged as "reserved".
    pub fn finalize(&self) {
        // SAFETY: Callback passed to C++ `cpp_root_resource_filter_populate_finalized_ranges`.
        // `filter` receives the context pointer passed below, which is a valid reference to `self`.
        unsafe extern "C" fn add_range_cb(filter: &RootResourceFilter, base: u64, size: u64) {
            let status = filter.mmio_deny.add_region(RegionSpan { base, size }, AllowOverlap::No);
            // If we cannot add the range to our set of regions to deny, it can only be
            // because we failed a heap allocation which should be impossible at this
            // point. If it does happen, panic. We cannot run if we cannot enforce the
            // deny list.
            assert_eq!(status, Ok(()));
        }

        // SAFETY: `cpp_root_resource_filter_populate_finalized_ranges` invokes the callback
        // synchronously.
        unsafe {
            cpp_root_resource_filter_populate_finalized_ranges(
                ptr::from_ref(self).cast(),
                add_range_cb,
            );
        }

        // Dump the deny list at spew level for debugging purposes.
        if dprintf_enabled(SPEW) {
            dprintf!(SPEW, "Final MMIO Deny list is:\n");
            self.mmio_deny.walk_available_regions(|region| {
                dprintf!(
                    SPEW,
                    "Region [{:#x}, {:#x})\n",
                    region.base(),
                    region.base() + region.size()
                );
                true
            });
            #[cfg(target_arch = "x86_64")]
            {
                self.ioport_deny.walk_available_regions(|region| {
                    dprintf!(
                        SPEW,
                        "IoPort [{:#x}, {:#x})\n",
                        region.base(),
                        region.base() + region.size()
                    );
                    true
                });
            }
        }
    }
}

// C ABI exports matching include/lib/root_resource_filter.h

/// Called by platform specific code to add a range to a specific resource type's
/// deny list. Must be called after global .ctors, heap initialization, and
/// after blocking is permitted. Once added to the deny list, resource ranges
/// which intersect any of the denied ranges may not be created, even with the
/// root resource. This is primarily used to ensure that even user-mode code may
/// not gain direct access to RAM, or to other kernel exclusive resources such as
/// the interrupt controller or IOMMU.
///
/// In the case of MMIO, automatic page rounding will be applied, as we cannot
/// restrict access to only part of a page of MMIO.
#[unsafe(no_mangle)]
pub extern "C" fn root_resource_filter_add_deny_region(
    base: usize,
    size: usize,
    kind: zx_rsrc_kind_t,
) {
    // We only enforce deny regions for MMIO right now. In the future, if someone
    // wants to limit other regions as well (perhaps the I/O port space for x64),
    // they need to come back here and add another RegionAllocator instance to
    // enforce the rules for the new zone.
    RootResourceFilter::global().add_deny_region(base, size, kind);
}

/// Called by object/resource code to check whether or not a resource of the
/// specified range and kind may be created. This restriction applies even to
/// users with access to the root resource.
pub fn root_resource_filter_can_access_region(
    base: usize,
    size: usize,
    kind: zx_rsrc_kind_t,
) -> bool {
    // Keep track of the number of regions that we end up denying. Typically, in
    // a properly operating system (aside from explicit tests) this should be 0.
    // Anything else probably indicates either malice or a bug somewhere.
    if !RootResourceFilter::global().is_region_allowed(base, size, kind) {
        ltracef!(
            "WARNING - Denying range request [{:016x}, {:016x}) kind ({})\n",
            base,
            base + size,
            kind
        );
        RESOURCE_RANGES_DENIED.add(1);
        return false;
    }

    true
}

fn finalize_root_resource_filter(_level: init::LkInitLevel) {
    RootResourceFilter::global().finalize();
}

init::lk_init_hook!(
    finalize_root_resource_filter,
    finalize_root_resource_filter,
    init::LkInitLevel(init::LK_INIT_LEVEL_USER.0 - 1)
);

/// Kernel unit tests for `RootResourceFilter`.
#[cfg(ktest)]
#[unittest::suite(name = "root_resource_filter_rust")]
mod tests {
    use super::RootResourceFilter;
    use pin_init::stack_pin_init;
    use unittest::{expect_eq, expect_false, expect_true};
    use zr::static_assert;
    use zx_types::{
        ZX_RSRC_KIND_IOPORT, ZX_RSRC_KIND_IRQ, ZX_RSRC_KIND_MMIO, ZX_RSRC_KIND_SMC,
        ZX_RSRC_KIND_SYSTEM, zx_rsrc_kind_t,
    };

    const ZX_RSRC_KIND_COUNT: u32 = ZX_RSRC_KIND_SYSTEM + 1;

    /// Test root_resource_filter default behaviour.
    #[test]
    fn test_default_behaviour() {
        stack_pin_init!(let filter = RootResourceFilter::init());

        // Start with asking for access to all of the various resource range kinds.
        // None of these requests should be denied, not even the ones which have no
        // meaningful concept of "range" associated with this. Unless explicitly
        // disallowed, all requests should default to OK.
        const RESOURCE_KINDS: [zx_rsrc_kind_t; 5] = [
            ZX_RSRC_KIND_MMIO,
            ZX_RSRC_KIND_IRQ,
            ZX_RSRC_KIND_IOPORT,
            ZX_RSRC_KIND_SMC,
            ZX_RSRC_KIND_SYSTEM,
        ];

        // Make sure that if someone adds new resource type, that someone comes back
        // here and adds it to this test.
        static_assert!(RESOURCE_KINDS.len() == (ZX_RSRC_KIND_COUNT - 1) as usize);

        for kind in RESOURCE_KINDS {
            expect_true!(filter.is_region_allowed(0, 1, kind));
        }
    }

    /// Test root_resource_filter (non-MMIO).
    #[test]
    fn test_non_mmio() {
        stack_pin_init!(let filter = RootResourceFilter::init());

        // Now manually add some ranges to the set of ranges to be denied. Test both
        // before and after to make sure that the ranges are allowed before they have
        // been added to the filter, and are properly denied afterwards.
        const RANGE_SIZE: usize = 128;
        struct TestVector {
            base: usize,
            size: usize,
            kind: zx_rsrc_kind_t,
        }
        let test_vectors = [
            TestVector { base: 0x0040, size: RANGE_SIZE, kind: ZX_RSRC_KIND_IOPORT },
            TestVector { base: 0x01c0, size: RANGE_SIZE, kind: ZX_RSRC_KIND_IOPORT },
            TestVector { base: 0x70ef, size: RANGE_SIZE, kind: ZX_RSRC_KIND_IOPORT },
            TestVector { base: 0x80ef, size: RANGE_SIZE, kind: ZX_RSRC_KIND_SMC },
            TestVector { base: 0x90ef, size: RANGE_SIZE, kind: ZX_RSRC_KIND_SMC },
        ];

        for pass in 0..2 {
            const TEST_SIZE: usize = 16;
            // Test range size must be at least twice as small as the test vector deny-range size.
            static_assert!((TEST_SIZE * 2) < RANGE_SIZE);

            for v in &test_vectors {
                // Entirely before and entirely after ranges should always pass.
                expect_true!(filter.is_region_allowed(v.base - TEST_SIZE, TEST_SIZE / 2, v.kind));
                expect_true!(filter.is_region_allowed(v.base + RANGE_SIZE, TEST_SIZE / 2, v.kind));

                // Now check ranges which overlap the start, overlap the end, and are
                // entirely contained within the deny ranges. These should succeed on the
                // first pass, but fail on the second (after we have added the deny-ranges
                // to the filter), or if the kind of range is SMC (currently the
                // deny list does not yet apply to the SMC domain).
                #[cfg(target_arch = "x86_64")]
                let expected = (pass == 0) || (v.kind == ZX_RSRC_KIND_SMC);
                #[cfg(not(target_arch = "x86_64"))]
                let expected = (pass == 0) || (v.kind != ZX_RSRC_KIND_MMIO);

                expect_eq!(
                    expected,
                    filter.is_region_allowed(v.base - (TEST_SIZE / 2), TEST_SIZE, v.kind)
                );
                expect_eq!(
                    expected,
                    filter.is_region_allowed(v.base + TEST_SIZE, TEST_SIZE, v.kind)
                );
                expect_eq!(
                    expected,
                    filter.is_region_allowed(
                        v.base + RANGE_SIZE - (TEST_SIZE / 2),
                        TEST_SIZE,
                        v.kind
                    )
                );
            }

            // If this was the first pass, add in our deny ranges.
            if pass == 0 {
                for v in &test_vectors {
                    filter.add_deny_region(v.base, v.size, v.kind);
                }
            }
        }
    }

    /// Test root_resource_filter (MMIO).
    #[test]
    fn test_mmio() {
        stack_pin_init!(let filter = RootResourceFilter::init());

        const HALF_PAGE_SIZE: usize = page::SIZE / 2;
        const PAGE_SIZE_X2: usize = page::SIZE * 2;
        const PAGE_SIZE_X3: usize = page::SIZE * 3;
        const PAGE_SIZE_X4: usize = page::SIZE * 4;

        // By default, [0, 0x4000) should be allowed.
        expect_true!(filter.is_region_allowed(0, PAGE_SIZE_X4, ZX_RSRC_KIND_MMIO));

        // Check that we can indeed deny [0, PAGE_SIZE)
        filter.add_deny_region(0, page::SIZE, ZX_RSRC_KIND_MMIO);
        expect_false!(filter.is_region_allowed(0, page::SIZE, ZX_RSRC_KIND_MMIO));
        expect_false!(filter.is_region_allowed(0, HALF_PAGE_SIZE, ZX_RSRC_KIND_MMIO));
        expect_false!(filter.is_region_allowed(HALF_PAGE_SIZE, HALF_PAGE_SIZE, ZX_RSRC_KIND_MMIO));
        expect_true!(filter.is_region_allowed(page::SIZE, PAGE_SIZE_X3, ZX_RSRC_KIND_MMIO));

        // With page rounding, denying [PAGE_SIZE + 0x100, PAGE_SIZE * 2) should be the same as
        // denying [PAGE_SIZE, PAGE_SIZE * 2), after which [PAGE_SIZE * 2, PAGE_SIZE * 4) should
        // still be allowed.
        filter.add_deny_region(page::SIZE + 0x100, page::SIZE - 0x100, ZX_RSRC_KIND_MMIO);
        expect_false!(filter.is_region_allowed(page::SIZE, page::SIZE, ZX_RSRC_KIND_MMIO));
        expect_false!(filter.is_region_allowed(page::SIZE, 0x100, ZX_RSRC_KIND_MMIO));
        expect_false!(filter.is_region_allowed(page::SIZE, HALF_PAGE_SIZE, ZX_RSRC_KIND_MMIO));
        expect_false!(filter.is_region_allowed(
            page::SIZE + HALF_PAGE_SIZE,
            page::SIZE - HALF_PAGE_SIZE,
            ZX_RSRC_KIND_MMIO
        ));
        expect_true!(filter.is_region_allowed(PAGE_SIZE_X2, PAGE_SIZE_X2, ZX_RSRC_KIND_MMIO));

        // With page rounding, denying [PAGE_SIZE * 2, PAGE_SIZE * 2 - 0x100) should be the same as
        // denying [PAGE_SIZE * 2, PAGE_SIZE * 3), after which [PAGE_SIZE * 3, PAGE_SIZE * 4) should
        // still be allowed.
        filter.add_deny_region(PAGE_SIZE_X2, page::SIZE - 0x100, ZX_RSRC_KIND_MMIO);
        expect_false!(filter.is_region_allowed(
            PAGE_SIZE_X2,
            page::SIZE - 0x100,
            ZX_RSRC_KIND_MMIO
        ));
        expect_false!(filter.is_region_allowed(PAGE_SIZE_X2 - 0x100, 0x100, ZX_RSRC_KIND_MMIO));
        expect_false!(filter.is_region_allowed(PAGE_SIZE_X2, HALF_PAGE_SIZE, ZX_RSRC_KIND_MMIO));
        expect_false!(filter.is_region_allowed(
            PAGE_SIZE_X2 + HALF_PAGE_SIZE,
            HALF_PAGE_SIZE,
            ZX_RSRC_KIND_MMIO
        ));
        expect_true!(filter.is_region_allowed(PAGE_SIZE_X3, page::SIZE, ZX_RSRC_KIND_MMIO));

        // With page rounding, denying [PAGE_SIZE * 3 + 0x100, PAGE_SIZE * 4 - 0x100) should be
        // the same as denying [PAGE_SIZE * 3, PAGE_SIZE * 4), after which all of
        // [0x0, PAGE_SIZE * 4) should still be denied.
        filter.add_deny_region(PAGE_SIZE_X3 + 0x100, page::SIZE - 0x200, ZX_RSRC_KIND_MMIO);
        expect_false!(filter.is_region_allowed(PAGE_SIZE_X3, page::SIZE, ZX_RSRC_KIND_MMIO));
        expect_false!(filter.is_region_allowed(PAGE_SIZE_X4 - 0x100, 0x100, ZX_RSRC_KIND_MMIO));
        expect_false!(filter.is_region_allowed(PAGE_SIZE_X3, HALF_PAGE_SIZE, ZX_RSRC_KIND_MMIO));
        expect_false!(filter.is_region_allowed(
            PAGE_SIZE_X3 + HALF_PAGE_SIZE,
            HALF_PAGE_SIZE,
            ZX_RSRC_KIND_MMIO
        ));
        expect_false!(filter.is_region_allowed(0x0, PAGE_SIZE_X4, ZX_RSRC_KIND_MMIO));
    }
}
