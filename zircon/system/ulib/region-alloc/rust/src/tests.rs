// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fbl::HasRefCount;
use pin_init::stack_pin_init;
use zx_status::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestFlavor {
    UsePool,
    UseHeap,
}

const REGION_POOL_MAX_SIZE: usize = RegionAllocator::REGION_POOL_SLAB_SIZE * 2;
const OOM_RANGE_LIMIT: usize = 1000;

const GOOD_MERGE_REGION_BASE: u64 = 0x3000000000000000;
const GOOD_MERGE_REGION_SIZE: u64 = 16 << 10;

const BAD_MERGE_REGION_BASE: u64 = 0x4000000000000000;
const BAD_MERGE_REGION_SIZE: u64 = 16 << 10;

const GOOD_REGIONS: &[RegionSpan] = &[
    RegionSpan { base: 0x10000000, size: 256 << 10 },
    RegionSpan { base: 0x20000000 - 1 * (256 << 10), size: 256 << 10 },
    RegionSpan { base: 0x20000000 + 3 * (256 << 10), size: 256 << 10 },
    RegionSpan { base: 0x20000000, size: 256 << 10 },
    RegionSpan { base: 0x20000000 + 2 * (256 << 10), size: 256 << 10 },
    RegionSpan { base: 0x20000000 + 1 * (256 << 10), size: 256 << 10 },
    RegionSpan { base: 0x1000000000000000, size: 256 << 10 },
    RegionSpan { base: 0x2000000000000000, size: 256 << 10 },
];

const BAD_REGIONS: &[RegionSpan] = &[
    RegionSpan { base: 0x10000000 - (256 << 10) + 1, size: 256 << 10 },
    RegionSpan { base: 0x10000000 - 1, size: 256 << 10 },
    RegionSpan { base: 0x10000000 + (256 << 10) - 1, size: 256 << 10 },
    RegionSpan { base: 0x10000000 - 1, size: 512 << 10 },
    RegionSpan { base: 0x10000000 + 1, size: 128 << 10 },
    RegionSpan { base: 0x1000000000000000 - (256 << 10) + 1, size: 256 << 10 },
    RegionSpan { base: 0x1000000000000000 - 1, size: 256 << 10 },
    RegionSpan { base: 0x1000000000000000 + (256 << 10) - 1, size: 256 << 10 },
    RegionSpan { base: 0x1000000000000000 - 1, size: 512 << 10 },
    RegionSpan { base: 0x1000000000000000 + 1, size: 128 << 10 },
    RegionSpan { base: 0x2000000000000000 - (256 << 10) + 1, size: 256 << 10 },
    RegionSpan { base: 0x2000000000000000 - 1, size: 256 << 10 },
    RegionSpan { base: 0x2000000000000000 + (256 << 10) - 1, size: 256 << 10 },
    RegionSpan { base: 0x2000000000000000 - 1, size: 512 << 10 },
    RegionSpan { base: 0x2000000000000000 + 1, size: 128 << 10 },
    RegionSpan { base: 0xFFFFFFFFFFFFFFFF, size: 0x1 },
    RegionSpan { base: 0xFFFFFFFF00000000, size: 0x100000000 },
];

fn region_contains_region(contained_by: &RegionSpan, contained: &Region) -> bool {
    let contained_end = contained.base() + contained.size() - 1;
    let contained_by_end = contained_by.base + contained_by.size - 1;

    (contained.base() >= contained_by.base)
        && (contained_end >= contained_by.base)
        && (contained.base() <= contained_by_end)
        && (contained_end <= contained_by_end)
}

const ALLOC_BY_SIZE_SMALL_REGION_BASE: u64 = 0x0;
const ALLOC_BY_SIZE_SMALL_REGION_SIZE: u64 = 4 << 10;
const ALLOC_BY_SIZE_LARGE_REGION_BASE: u64 = 0x100000;
const ALLOC_BY_SIZE_LARGE_REGION_SIZE: u64 = 1 << 20;

const ALLOC_BY_SIZE_REGIONS: &[RegionSpan] = &[
    RegionSpan { base: ALLOC_BY_SIZE_SMALL_REGION_BASE, size: ALLOC_BY_SIZE_SMALL_REGION_SIZE },
    RegionSpan { base: ALLOC_BY_SIZE_LARGE_REGION_BASE, size: ALLOC_BY_SIZE_LARGE_REGION_SIZE },
];
const OK: Result<(), Status> = Ok(());
const NOT_FOUND: Result<(), Status> = Err(Status::NOT_FOUND);
const INVALID_ARGS: Result<(), Status> = Err(Status::INVALID_ARGS);

struct AllocBySizeAllocTest {
    size: u64,
    align: u64,
    res: Result<(), Status>,
    region: usize,
}

const ALLOC_BY_SIZE_TESTS: &[AllocBySizeAllocTest] = &[
    // Invalid parameter failures
    AllocBySizeAllocTest { size: 0x00000000, align: 0x00000001, res: INVALID_ARGS, region: 0 },
    AllocBySizeAllocTest { size: 0x00000001, align: 0x00000000, res: INVALID_ARGS, region: 0 },
    AllocBySizeAllocTest { size: 0x00000001, align: 0x00001001, res: INVALID_ARGS, region: 0 },
    // Initially unsatisfiable
    AllocBySizeAllocTest { size: 0x10000000, align: 0x00000001, res: NOT_FOUND, region: 0 },
    AllocBySizeAllocTest { size: 0x00005000, align: 0x10000000, res: NOT_FOUND, region: 0 },
    // Should succeed, all pulled from first chunk
    AllocBySizeAllocTest { size: 1 << 0, align: 1 << 1, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 1, align: 1 << 2, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 2, align: 1 << 3, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 3, align: 1 << 4, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 4, align: 1 << 5, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 5, align: 1 << 6, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 6, align: 1 << 7, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 7, align: 1 << 8, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 8, align: 1 << 9, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 9, align: 1 << 10, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 10, align: 1 << 11, res: OK, region: 0 },
    // Perform some allocations which are large enough that they can only be
    // satisfied with results from region 1.  Exercise the various range
    // splitting cases.
    AllocBySizeAllocTest { size: 4 << 10, align: 4 << 10, res: OK, region: 1 },
    AllocBySizeAllocTest { size: 4 << 10, align: 4 << 11, res: OK, region: 1 },
    AllocBySizeAllocTest { size: 0xfc000, align: 4 << 12, res: OK, region: 1 },
    // Repeat the small allocation pass again.  Because of the alignment
    // restrictions, the first pass should have fragmented the first region.
    // This pass should soak up those fragments.
    AllocBySizeAllocTest { size: 3, align: 1 << 0, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 1, align: 1 << 1, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 2, align: 1 << 2, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 3, align: 1 << 3, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 4, align: 1 << 4, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 5, align: 1 << 5, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 6, align: 1 << 6, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 7, align: 1 << 7, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 8, align: 1 << 8, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 9, align: 1 << 9, res: OK, region: 0 },
    AllocBySizeAllocTest { size: 1 << 10, align: 1 << 10, res: OK, region: 0 },
    // Region 0 should be exhausted at this point.  Asking for even one more
    // byte should give us an allocation from from region 1.
    AllocBySizeAllocTest { size: 1, align: 1, res: OK, region: 1 },
    // All that should be left in the pool is a 4k region and a 4k - 1 byte
    // region.  Ask for two 4k regions with arbitrary alignment.  The first
    // request should succeed while the second request should fail.
    AllocBySizeAllocTest { size: 4 << 10, align: 1, res: OK, region: 1 },
    AllocBySizeAllocTest { size: 4 << 10, align: 1, res: NOT_FOUND, region: 0 },
    // Finally, soak up the last of the space with a 0xFFF byte allocation.
    // Afterwards, we should be unable to allocate even a single byte
    AllocBySizeAllocTest { size: 0xFFF, align: 1, res: OK, region: 1 },
    AllocBySizeAllocTest { size: 1, align: 1, res: NOT_FOUND, region: 0 },
];

const ALLOC_SPECIFIC_REGION_BASE: u64 = 0x1000;
const ALLOC_SPECIFIC_REGION_SIZE: u64 = 4 << 10;

const ALLOC_SPECIFIC_REGIONS: &[RegionSpan] =
    &[RegionSpan { base: ALLOC_SPECIFIC_REGION_BASE, size: ALLOC_SPECIFIC_REGION_SIZE }];

struct AllocSpecificAllocTest {
    req: RegionSpan,
    res: Result<(), Status>,
}

const ALLOC_SPECIFIC_TESTS: &[AllocSpecificAllocTest] = &[
    // Invalid parameter failures
    AllocSpecificAllocTest {
        req: RegionSpan { base: 0x0000000000000000, size: 0x00 },
        res: INVALID_ARGS,
    },
    AllocSpecificAllocTest {
        req: RegionSpan { base: 0xffffffffffffffff, size: 0x01 },
        res: INVALID_ARGS,
    },
    AllocSpecificAllocTest {
        req: RegionSpan { base: 0xfffffffffffffff0, size: 0x20 },
        res: INVALID_ARGS,
    },
    // Bad requests
    AllocSpecificAllocTest { req: RegionSpan { base: 0x0800, size: 0x1 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x0fff, size: 0x100 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1f01, size: 0x100 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x2000, size: 0x1 }, res: NOT_FOUND },
    // Good requests
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1000, size: 0x100 }, res: OK },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1f00, size: 0x100 }, res: OK },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1700, size: 0x200 }, res: OK },
    // Requests which would have been good initially, but are bad now.
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1000, size: 0x100 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1080, size: 0x80 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x10ff, size: 0x1 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x10ff, size: 0x100 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1f00, size: 0x100 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1e01, size: 0x100 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1e81, size: 0x80 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1eff, size: 0x2 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1800, size: 0x100 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1880, size: 0x100 }, res: NOT_FOUND },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1780, size: 0x100 }, res: NOT_FOUND },
    // Soak up the remaining regions.  There should be 2 left.
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1100, size: 0x600 }, res: OK },
    AllocSpecificAllocTest { req: RegionSpan { base: 0x1900, size: 0x600 }, res: OK },
];

struct AllocAddOverlapTest {
    reg: RegionSpan,
    ovl: bool,
    cnt: usize,
    res: Result<(), Status>,
}

const ADD_OVERLAP_TESTS: &[AllocAddOverlapTest] = &[
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x10000, size: 0x1000 },
        ovl: false,
        cnt: 1,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x10000, size: 0x1000 },
        ovl: false,
        cnt: 1,
        res: INVALID_ARGS,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x10000, size: 0x1000 },
        ovl: true,
        cnt: 1,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0xF800, size: 0x800 },
        ovl: false,
        cnt: 1,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0xF800, size: 0x800 },
        ovl: true,
        cnt: 1,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x11000, size: 0x800 },
        ovl: false,
        cnt: 1,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x11000, size: 0x800 },
        ovl: true,
        cnt: 1,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0xF000, size: 0x801 },
        ovl: false,
        cnt: 1,
        res: INVALID_ARGS,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0xF000, size: 0x801 },
        ovl: true,
        cnt: 1,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x117FF, size: 0x801 },
        ovl: false,
        cnt: 1,
        res: INVALID_ARGS,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x117FF, size: 0x801 },
        ovl: true,
        cnt: 1,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0xE000, size: 0x5000 },
        ovl: false,
        cnt: 1,
        res: INVALID_ARGS,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0xE000, size: 0x5000 },
        ovl: true,
        cnt: 1,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x14000, size: 0x1000 },
        ovl: false,
        cnt: 2,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x16000, size: 0x1000 },
        ovl: false,
        cnt: 3,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x18000, size: 0x1000 },
        ovl: false,
        cnt: 4,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x1A000, size: 0x1000 },
        ovl: false,
        cnt: 5,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x1C000, size: 0x1000 },
        ovl: false,
        cnt: 6,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x12FFF, size: 0x1002 },
        ovl: false,
        cnt: 6,
        res: INVALID_ARGS,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x12FFF, size: 0x1002 },
        ovl: true,
        cnt: 5,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x15800, size: 0x3000 },
        ovl: false,
        cnt: 5,
        res: INVALID_ARGS,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x15800, size: 0x3000 },
        ovl: true,
        cnt: 4,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x18800, size: 0x3000 },
        ovl: false,
        cnt: 4,
        res: INVALID_ARGS,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0x18800, size: 0x3000 },
        ovl: true,
        cnt: 3,
        res: OK,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0xD000, size: 0x11000 },
        ovl: false,
        cnt: 3,
        res: INVALID_ARGS,
    },
    AllocAddOverlapTest {
        reg: RegionSpan { base: 0xD000, size: 0x11000 },
        ovl: true,
        cnt: 1,
        res: OK,
    },
];

struct AllocSubtractTest {
    reg: RegionSpan,
    add: bool,
    incomplete: bool,
    cnt: usize,
    res: bool,
}

const SUBTRACT_TESTS: &[AllocSubtractTest] = &[
    // Try to subtract a region while the allocator is empty.  This should fail unless we allow
    // incomplete subtraction.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: false,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: false,
        incomplete: true,
        cnt: 0,
        res: true,
    },
    // allow_incomplete == false
    // Tests where incomplete subtraction is not allowed.

    // Add a region, then subtract it out.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // Add a region, then trim the front of it.  Finally, cleanup by removing
    // the specific regions which should be left.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1800, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // Add a region, then trim the back of it.  Then cleanup.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1800, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // Add a region, then punch a hole in the middle of it. then cleanup.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1600, size: 0x400 },
        add: false,
        incomplete: false,
        cnt: 2,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x600 },
        add: false,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1A00, size: 0x600 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // Add a region, then fail to remove parts of it with a number of attempts
    // which would require trimming or splitting the region.  Then cleanup.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x800, size: 0x1000 },
        add: false,
        incomplete: false,
        cnt: 1,
        res: false,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1800, size: 0x1000 },
        add: false,
        incomplete: false,
        cnt: 1,
        res: false,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x800, size: 0x2000 },
        add: false,
        incomplete: false,
        cnt: 1,
        res: false,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // allow_incomplete == true
    // Tests where incomplete subtraction is allowed.  Start by repeating the
    // tests for allow_incomplete = false where success was expected.  These
    // should work too.

    // Add a region, then subtract it out.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: false,
        incomplete: true,
        cnt: 0,
        res: true,
    },
    // Add a region, then trim the front of it.  Finally, cleanup by removing
    // the specific regions which should be left.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x800 },
        add: false,
        incomplete: true,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1800, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // Add a region, then trim the back of it.  Then cleanup.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1800, size: 0x800 },
        add: false,
        incomplete: true,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // Add a region, then punch a hole in the middle of it. then cleanup.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1600, size: 0x400 },
        add: false,
        incomplete: true,
        cnt: 2,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x600 },
        add: false,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1A00, size: 0x600 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // Now try scenarios which only work when allow_incomplete is true.
    // Add a region, then trim the front.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x800, size: 0x1000 },
        add: false,
        incomplete: true,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1800, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // Add a region, then trim the back.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1800, size: 0x1000 },
        add: false,
        incomplete: true,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // Add a region, then consume the whole thing.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x800, size: 0x2000 },
        add: false,
        incomplete: true,
        cnt: 0,
        res: true,
    },
    // Add a bunch of separate regions, then consume them all using a subtract
    // which lines up perfectly with the beginning and the end of the regions.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x3000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 2,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x5000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 3,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x7000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 4,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x9000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 5,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0xA000 },
        add: false,
        incomplete: true,
        cnt: 0,
        res: true,
    },
    // Same as before, but this time, trim past the start
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x3000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 2,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x5000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 3,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x7000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 4,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x9000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 5,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x800, size: 0xA800 },
        add: false,
        incomplete: true,
        cnt: 0,
        res: true,
    },
    // Same as before, but this time, trim past the end
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x3000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 2,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x5000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 3,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x7000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 4,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x9000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 5,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0xA800 },
        add: false,
        incomplete: true,
        cnt: 0,
        res: true,
    },
    // Same as before, but this time, trim past both ends
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x3000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 2,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x5000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 3,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x7000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 4,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x9000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 5,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x800, size: 0xB000 },
        add: false,
        incomplete: true,
        cnt: 0,
        res: true,
    },
    // Same as before, but this time, don't consume all of the first region.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x3000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 2,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x5000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 3,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x7000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 4,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x9000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 5,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1800, size: 0x9800 },
        add: false,
        incomplete: true,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // Same as before, but this time, don't consume all of the last region.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x3000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 2,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x5000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 3,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x7000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 4,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x9000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 5,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x8800 },
        add: false,
        incomplete: true,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x9800, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
    // Same as before, but this time, don't consume all of the first or last regions.
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x3000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 2,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x5000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 3,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x7000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 4,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x9000, size: 0x1000 },
        add: true,
        incomplete: false,
        cnt: 5,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1800, size: 0x8000 },
        add: false,
        incomplete: true,
        cnt: 2,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x1000, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 1,
        res: true,
    },
    AllocSubtractTest {
        reg: RegionSpan { base: 0x9800, size: 0x800 },
        add: false,
        incomplete: false,
        cnt: 0,
        res: true,
    },
];

fn check_region_match_ref(test: &Region, expected: &RegionSpan) {
    assert_eq!(test.base(), expected.base);
    assert_eq!(test.size(), expected.size);
}

#[test]
fn test_region_pools() {
    stack_pin_init!(let alloc = RegionAllocator::init());

    let pool = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
    assert_eq!(pool.ref_count().ref_count_debug(), 1);

    {
        assert_eq!(alloc.add_region(RegionSpan { base: 0, size: 1024 }, AllowOverlap::No), Ok(()));

        let tmp = alloc.get_region_specific(RegionSpan { base: 128, size: 256 });
        assert!(tmp.is_ok());
        let tmp = tmp.unwrap();

        assert_eq!(alloc.set_region_pool(pool.clone()), Err(Status::BAD_STATE));

        drop(tmp);
        assert_eq!(alloc.set_region_pool(pool.clone()), Err(Status::BAD_STATE));

        alloc.reset();
    }

    assert_eq!(alloc.set_region_pool(pool.clone()), Ok(()));

    {
        stack_pin_init!(let alloc2 = RegionAllocator::init());
        assert_eq!(alloc2.set_region_pool(pool.clone()), Ok(()));
    }

    for region in GOOD_REGIONS {
        assert_eq!(alloc.add_region(*region, AllowOverlap::No), Ok(()));
    }

    let pool2 = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
    assert_eq!(alloc.set_region_pool(pool2.clone()), Err(Status::BAD_STATE));

    {
        let mut tmp = RegionSpan { base: GOOD_MERGE_REGION_BASE, size: GOOD_MERGE_REGION_SIZE };
        for _ in 0..OOM_RANGE_LIMIT {
            assert_eq!(alloc.add_region(tmp, AllowOverlap::No), Ok(()));
            tmp.base += tmp.size;
        }
    }

    for region in BAD_REGIONS {
        assert_eq!(alloc.add_region(*region, AllowOverlap::No), Err(Status::INVALID_ARGS));
    }

    {
        let mut tmp = RegionSpan { base: BAD_MERGE_REGION_BASE, size: BAD_MERGE_REGION_SIZE };
        let mut oom_reached = false;
        for _ in 0..OOM_RANGE_LIMIT {
            let res = alloc.add_region(tmp, AllowOverlap::No);
            if res != Ok(()) {
                assert_eq!(res, Err(Status::NO_MEMORY));
                oom_reached = true;
                break;
            }
            tmp.base += tmp.size + 1;
        }
        assert!(oom_reached);
    }

    alloc.reset();

    assert_eq!(alloc.set_region_pool(pool2), Ok(()));
}

fn alloc_by_size_helper(flavor: TestFlavor) {
    stack_pin_init!(let alloc = RegionAllocator::init());
    if flavor == TestFlavor::UsePool {
        let pool = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
        alloc.set_region_pool(pool).unwrap();
    }

    for region in ALLOC_BY_SIZE_REGIONS {
        assert_eq!(alloc.add_region(*region, AllowOverlap::No), Ok(()));
    }

    let mut regions: [Option<UniquePtr<Region>>; ALLOC_BY_SIZE_TESTS.len()] =
        core::array::from_fn(|_| None);

    for (i, test) in ALLOC_BY_SIZE_TESTS.iter().enumerate() {
        let res = alloc.get_region(test.size, test.align);

        if test.res == OK {
            assert!(res.is_ok());
            let r = res.unwrap();
            assert!(region_contains_region(&ALLOC_BY_SIZE_REGIONS[test.region], &r));
            assert_eq!(r.base() & (test.align - 1), 0);
            regions[i] = Some(r);
        } else {
            assert_eq!(res.err(), test.res.err());
            assert!(regions[i].is_none());
        }
    }
}

#[test]
fn test_alloc_by_size_from_pool() {
    alloc_by_size_helper(TestFlavor::UsePool);
}

#[test]
fn test_alloc_by_size_from_heap() {
    alloc_by_size_helper(TestFlavor::UseHeap);
}

fn alloc_specific_helper(flavor: TestFlavor) {
    stack_pin_init!(let alloc = RegionAllocator::init());
    if flavor == TestFlavor::UsePool {
        let pool = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
        alloc.set_region_pool(pool).unwrap();
    }

    for region in ALLOC_SPECIFIC_REGIONS {
        assert_eq!(alloc.add_region(*region, AllowOverlap::No), Ok(()));
    }

    let mut regions: [Option<UniquePtr<Region>>; ALLOC_SPECIFIC_TESTS.len()] =
        core::array::from_fn(|_| None);

    for (i, test) in ALLOC_SPECIFIC_TESTS.iter().enumerate() {
        let res = alloc.get_region_specific(test.req);

        if test.res == OK {
            assert!(res.is_ok());
            let r = res.unwrap();
            assert_eq!(r.base(), test.req.base);
            assert_eq!(r.size(), test.req.size);
            regions[i] = Some(r);
        } else {
            assert_eq!(res.err(), test.res.err());
            assert!(regions[i].is_none());
        }
    }
}

#[test]
fn test_alloc_specific_from_pool() {
    alloc_specific_helper(TestFlavor::UsePool);
}

#[test]
fn test_alloc_specific_from_heap() {
    alloc_specific_helper(TestFlavor::UseHeap);
}

fn add_overlap_helper(flavor: TestFlavor) {
    stack_pin_init!(let alloc = RegionAllocator::init());
    if flavor == TestFlavor::UsePool {
        let pool = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
        alloc.set_region_pool(pool).unwrap();
    }

    for test in ADD_OVERLAP_TESTS {
        let res =
            alloc.add_region(test.reg, if test.ovl { AllowOverlap::Yes } else { AllowOverlap::No });
        assert_eq!(res, test.res);
        assert_eq!(alloc.available_region_count(), test.cnt);
    }
}

#[test]
fn test_add_overlap_from_pool() {
    add_overlap_helper(TestFlavor::UsePool);
}

#[test]
fn test_add_overlap_from_heap() {
    add_overlap_helper(TestFlavor::UseHeap);
}

fn subtract_helper(flavor: TestFlavor) {
    stack_pin_init!(let alloc = RegionAllocator::init());
    if flavor == TestFlavor::UsePool {
        let pool = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
        alloc.set_region_pool(pool).unwrap();
    }

    for test in SUBTRACT_TESTS {
        let res = if test.add {
            alloc.add_region(test.reg, AllowOverlap::No)
        } else {
            alloc.subtract_region(
                test.reg,
                if test.incomplete { AllowIncomplete::Yes } else { AllowIncomplete::No },
            )
        };
        if test.res {
            assert_eq!(res, Ok(()));
        } else {
            assert_eq!(res, Err(Status::INVALID_ARGS));
        }
        assert_eq!(alloc.available_region_count(), test.cnt);
    }
}

#[test]
fn test_subtract_from_pool() {
    subtract_helper(TestFlavor::UsePool);
}

#[test]
fn test_subtract_from_heap() {
    subtract_helper(TestFlavor::UseHeap);
}

fn allocated_walk_helper(flavor: TestFlavor) {
    stack_pin_init!(let alloc = RegionAllocator::init());
    if flavor == TestFlavor::UsePool {
        let pool = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
        alloc.set_region_pool(pool).unwrap();
    }

    let test_regions = &[
        RegionSpan { base: 0x00000000, size: 1 << 20 },
        RegionSpan { base: 0x10000000, size: 1 << 20 },
        RegionSpan { base: 0x20000000, size: 1 << 20 },
        RegionSpan { base: 0x30000000, size: 1 << 20 },
        RegionSpan { base: 0x40000000, size: 1 << 20 },
        RegionSpan { base: 0x50000000, size: 1 << 20 },
        RegionSpan { base: 0x60000000, size: 1 << 20 },
        RegionSpan { base: 0x70000000, size: 1 << 20 },
        RegionSpan { base: 0x80000000, size: 1 << 20 },
        RegionSpan { base: 0x90000000, size: 1 << 20 },
    ];
    let r_cnt = test_regions.len();

    alloc.add_region(RegionSpan { base: 0, size: u64::MAX }, AllowOverlap::No).unwrap();

    let mut r: [Option<UniquePtr<Region>>; 10] = Default::default();
    for i in 0..r_cnt {
        r[i] = Some(alloc.get_region_specific(test_regions[i]).unwrap());
    }

    let mut pos = 0;
    let mut end = 0;

    let mut cb = |region: &Region| -> bool {
        check_region_match_ref(region, &test_regions[pos]);
        pos += 1;
        if end > 0 { pos != end } else { true }
    };

    alloc.walk_allocated_regions(&mut cb);
    assert_eq!(r_cnt, pos);

    use rand::{RngExt as _, SeedableRng as _};
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);

    for _ in 0..1024 {
        pos = 0;
        end = (rng.random::<u32>() as usize % r_cnt) + 1;
        let mut cb_early = |region: &Region| -> bool {
            check_region_match_ref(region, &test_regions[pos]);
            pos += 1;
            pos != end
        };
        alloc.walk_allocated_regions(&mut cb_early);
        assert_eq!(pos, end);
    }
}

#[test]
fn test_allocated_walk_from_pool() {
    allocated_walk_helper(TestFlavor::UsePool);
}

#[test]
fn test_allocated_walk_from_heap() {
    allocated_walk_helper(TestFlavor::UseHeap);
}

fn test_region_helper(flavor: TestFlavor) {
    stack_pin_init!(let alloc = RegionAllocator::init());
    if flavor == TestFlavor::UsePool {
        let pool = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
        alloc.set_region_pool(pool).unwrap();
    }

    let test_regions = &[
        RegionSpan { base: 0x1000, size: 0x2000 },
        RegionSpan { base: 0x4000, size: 0x2000 },
        RegionSpan { base: 0x8000, size: 0x2000 },
    ];

    struct AllocatedRegion {
        region: RegionSpan,
        ptr: Option<UniquePtr<Region>>,
    }

    let mut allocated_regions = [
        AllocatedRegion { region: RegionSpan { base: 0x1000, size: 0x1000 }, ptr: None },
        AllocatedRegion { region: RegionSpan { base: 0x4800, size: 0x1000 }, ptr: None },
        AllocatedRegion { region: RegionSpan { base: 0x9000, size: 0x1000 }, ptr: None },
    ];

    for r in test_regions {
        alloc.add_region(*r, AllowOverlap::No).unwrap();
    }

    for ar in &mut allocated_regions {
        ar.ptr = Some(alloc.get_region_specific(ar.region).unwrap());
    }

    struct TestVector {
        region: RegionSpan,
        ai: bool,
        ac: bool,
        vi: bool,
        vc: bool,
    }

    let test_vectors = &[
        TestVector {
            region: RegionSpan { base: 0x0000, size: 0xF000 },
            ai: true,
            ac: false,
            vi: true,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x0000, size: 0x100 },
            ai: false,
            ac: false,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x0FF0, size: 0x10 },
            ai: false,
            ac: false,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x0FF1, size: 0x10 },
            ai: true,
            ac: false,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x1000, size: 0x10 },
            ai: true,
            ac: true,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x1010, size: 0x10 },
            ai: true,
            ac: true,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x1FF0, size: 0x10 },
            ai: true,
            ac: true,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x1FF8, size: 0x10 },
            ai: true,
            ac: false,
            vi: true,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x2000, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x2010, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x2FF0, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x2FF8, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x3000, size: 0x10 },
            ai: false,
            ac: false,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x3FF0, size: 0x10 },
            ai: false,
            ac: false,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x3FF1, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x4000, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x4010, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x47F0, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x47F8, size: 0x10 },
            ai: true,
            ac: false,
            vi: true,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x4800, size: 0x10 },
            ai: true,
            ac: true,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x4900, size: 0x10 },
            ai: true,
            ac: true,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x57F0, size: 0x10 },
            ai: true,
            ac: true,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x57F8, size: 0x10 },
            ai: true,
            ac: false,
            vi: true,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x5800, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x5900, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x5FF0, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x5FF8, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x6000, size: 0x10 },
            ai: false,
            ac: false,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x7FF0, size: 0x10 },
            ai: false,
            ac: false,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x7FF1, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x8000, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x8010, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x8FF0, size: 0x10 },
            ai: false,
            ac: false,
            vi: true,
            vc: true,
        },
        TestVector {
            region: RegionSpan { base: 0x8FF8, size: 0x10 },
            ai: true,
            ac: false,
            vi: true,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x9000, size: 0x10 },
            ai: true,
            ac: true,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x9010, size: 0x10 },
            ai: true,
            ac: true,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x9FF0, size: 0x10 },
            ai: true,
            ac: true,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0x9FF8, size: 0x10 },
            ai: true,
            ac: false,
            vi: false,
            vc: false,
        },
        TestVector {
            region: RegionSpan { base: 0xA000, size: 0x10 },
            ai: false,
            ac: false,
            vi: false,
            vc: false,
        },
    ];

    for tv in test_vectors {
        assert_eq!(alloc.test_region_intersects(tv.region, TestRegionSet::Allocated), Ok(tv.ai));
        assert_eq!(alloc.test_region_contained_by(tv.region, TestRegionSet::Allocated), Ok(tv.ac));
        assert_eq!(alloc.test_region_intersects(tv.region, TestRegionSet::Available), Ok(tv.vi));
        assert_eq!(alloc.test_region_contained_by(tv.region, TestRegionSet::Available), Ok(tv.vc));
    }
}

#[test]
fn test_region_from_pool() {
    test_region_helper(TestFlavor::UsePool);
}

#[test]
fn test_region_from_heap() {
    test_region_helper(TestFlavor::UseHeap);
}

fn invalid_region_intersects_helper(flavor: TestFlavor) {
    stack_pin_init!(let alloc = RegionAllocator::init());
    if flavor == TestFlavor::UsePool {
        let pool = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
        alloc.set_region_pool(pool).unwrap();
    }

    const AVAIL_SIZE: u64 = 0x10000;
    let avail_base: u64 = u64::MAX - AVAIL_SIZE;
    alloc.add_region(RegionSpan { base: avail_base, size: AVAIL_SIZE }, AllowOverlap::No).unwrap();

    let test_base: u64 = avail_base - 1;
    let test_size: u64 = u64::MAX - test_base;
    let mut test_region = RegionSpan { base: test_base, size: test_size };

    assert_eq!(alloc.test_region_intersects(test_region, TestRegionSet::Available), Ok(true));

    test_region.size += 1;
    assert_eq!(
        alloc.test_region_intersects(test_region, TestRegionSet::Available),
        Err(Status::INVALID_ARGS)
    );

    test_region.size += 4096;
    assert_eq!(
        alloc.test_region_intersects(test_region, TestRegionSet::Available),
        Err(Status::INVALID_ARGS)
    );

    test_region.size = 0;
    assert_eq!(
        alloc.test_region_intersects(test_region, TestRegionSet::Available),
        Err(Status::INVALID_ARGS)
    );
}

#[test]
fn test_invalid_region_intersects_from_pool() {
    invalid_region_intersects_helper(TestFlavor::UsePool);
}

#[test]
fn test_invalid_region_intersects_from_heap() {
    invalid_region_intersects_helper(TestFlavor::UseHeap);
}

fn invalid_region_contained_by_helper(flavor: TestFlavor) {
    stack_pin_init!(let alloc = RegionAllocator::init());
    if flavor == TestFlavor::UsePool {
        let pool = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
        alloc.set_region_pool(pool).unwrap();
    }

    const AVAIL_SIZE: u64 = 0x10000;
    let avail_base: u64 = u64::MAX - AVAIL_SIZE;
    alloc.add_region(RegionSpan { base: avail_base, size: AVAIL_SIZE }, AllowOverlap::No).unwrap();

    let test_base: u64 = avail_base + 1;
    let test_size: u64 = u64::MAX - test_base;
    let mut test_region = RegionSpan { base: test_base, size: test_size };

    assert_eq!(alloc.test_region_contained_by(test_region, TestRegionSet::Available), Ok(true));

    test_region.size += 1;
    assert_eq!(
        alloc.test_region_contained_by(test_region, TestRegionSet::Available),
        Err(Status::INVALID_ARGS)
    );

    test_region.size += 4096;
    assert_eq!(
        alloc.test_region_contained_by(test_region, TestRegionSet::Available),
        Err(Status::INVALID_ARGS)
    );

    test_region.size = 0;
    assert_eq!(
        alloc.test_region_contained_by(test_region, TestRegionSet::Available),
        Err(Status::INVALID_ARGS)
    );
}

#[test]
fn test_invalid_region_contained_by_from_pool() {
    invalid_region_contained_by_helper(TestFlavor::UsePool);
}

#[test]
fn test_invalid_region_contained_by_from_heap() {
    invalid_region_contained_by_helper(TestFlavor::UseHeap);
}

#[test]
fn test_init_with_pool() {
    let pool = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
    stack_pin_init!(let alloc = RegionAllocator::init_with_pool(pool));
    assert!(alloc.has_region_pool());
}

#[test]
fn test_get_region_pointer_aligned() {
    stack_pin_init!(let alloc = RegionAllocator::init());
    let pool = RegionPool::create(REGION_POOL_MAX_SIZE).unwrap();
    alloc.set_region_pool(pool).unwrap();

    alloc.add_region(RegionSpan { base: 1024, size: 1024 }, AllowOverlap::No).unwrap();

    // Allocate 100 bytes, pointer-aligned
    let r = alloc.get_region_pointer_aligned(100).unwrap();
    let ptr_size = core::mem::size_of::<*const ()>() as u64;
    assert_eq!(r.base() % ptr_size, 0);
    assert_eq!(r.size(), 100);
}
