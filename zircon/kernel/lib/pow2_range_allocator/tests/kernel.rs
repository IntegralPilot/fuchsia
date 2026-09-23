// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Ported from zircon/kernel/lib/pow2_range_allocator/pow2_range_allocator_tests.cc

/// Test suite for the Rust `Pow2RangeAllocator` implementation.
#[cfg(ktest)]
#[unittest::suite(name = "pow2_range_allocator")]
mod tests {
    use pin_init::stack_pin_init;
    use pow2_range_allocator::Pow2RangeAllocator;
    use unittest::{assert_eq, assert_ge, assert_lt, assert_ok, unwrap_ok};
    use zx_status::Status;

    /// Tests that `init` accepts only power of two maximum allocation sizes.
    #[test]
    fn init_free() {
        // The max_alloc_size must be a power of two. Test all those first.
        let mut size: u32 = 1;
        while size != 0 {
            stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
            assert_ok!(p2ra.init(size));
            p2ra.free();
            size <<= 1;
        }

        // Non-power of two sizes should fail.
        for size in [0u32, 3, 7, 11, 12, 48] {
            stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
            assert_eq!(Status::result_into_raw(p2ra.init(size)), Status::INVALID_ARGS.into_raw());
        }
    }

    /// Tests the validation and overlap detection performed by `add_range`.
    #[test]
    fn add_range() {
        {
            // Adding a range that wraps a u32 should fail.
            stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
            assert_ok!(p2ra.init(64));
            assert_eq!(
                Status::result_into_raw(p2ra.add_range(1u32 << 31, 1u32 << 31)),
                Status::INVALID_ARGS.into_raw()
            );
            p2ra.free();
        }

        {
            // Adding a zero-length range should fail.
            stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
            assert_ok!(p2ra.init(64));
            assert_eq!(
                Status::result_into_raw(p2ra.add_range(32, 0)),
                Status::INVALID_ARGS.into_raw()
            );
            p2ra.free();
        }

        {
            // Adding the same range twice should fail.
            stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
            assert_ok!(p2ra.init(64));
            assert_ok!(p2ra.add_range(0, 32));
            assert_eq!(
                Status::result_into_raw(p2ra.add_range(0, 32)),
                Status::ALREADY_EXISTS.into_raw()
            );
            p2ra.free();
        }

        {
            // Adding a subrange of an already-added range should fail.
            stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
            assert_ok!(p2ra.init(64));
            assert_ok!(p2ra.add_range(0, 32));
            assert_ok!(p2ra.add_range(32, 16));
            assert_eq!(
                Status::result_into_raw(p2ra.add_range(0, 16)),
                Status::ALREADY_EXISTS.into_raw()
            );
            p2ra.free();
        }

        {
            // Adding a super-range of an already range should fail.
            stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
            assert_ok!(p2ra.init(64));
            assert_ok!(p2ra.add_range(0, 16));
            assert_eq!(
                Status::result_into_raw(p2ra.add_range(0, 32)),
                Status::ALREADY_EXISTS.into_raw()
            );
            p2ra.free();
        }

        {
            // Adding adjacent ranges should succeed.
            stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
            assert_ok!(p2ra.init(64));
            assert_ok!(p2ra.add_range(0, 16));
            assert_ok!(p2ra.add_range(16, 16));
            p2ra.free();
        }

        {
            // Adding a range larger than the initialized size should succeed.
            stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
            assert_ok!(p2ra.init(64));
            assert_ok!(p2ra.add_range(0, 128));
            p2ra.free();
        }

        {
            // Adding a bunch of ranges should succeed.
            stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
            assert_ok!(p2ra.init(128));
            let mut size: u32 = 1;
            while size < 128 {
                assert_ok!(p2ra.add_range(size, size));
                size *= 2;
            }
            p2ra.free();
        }
    }

    /// Tests allocation, splitting, merging and fragmentation behavior of `allocate_range`.
    #[test]
    fn allocate_range() {
        // The C++ test also verified that `AllocateRange(4, nullptr)` returns ZX_ERR_INVALID_ARGS.
        // The Rust API returns `Result<u32, Status>` instead of writing through an out parameter,
        // so that case cannot be expressed and is intentionally not ported.

        {
            // Allocating a range with a non-power-of-2 length should fail.
            stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
            assert_ok!(p2ra.init(64));
            assert_ok!(p2ra.add_range(0, 64));
            for size in [0u32, 3, 3, 7, 48] {
                assert_eq!(
                    Status::result_into_raw(p2ra.allocate_range(size).map(|_| ())),
                    Status::INVALID_ARGS.into_raw()
                );
            }
            p2ra.free();
        }

        {
            // Ranges should be distinct.
            for range_length in [1u32, 4, 16] {
                const NUMBER_OF_RANGES: u32 = 64;
                let total_size = NUMBER_OF_RANGES * range_length;
                stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
                assert_ok!(p2ra.init(total_size));
                assert_ok!(p2ra.add_range(0, total_size));
                let mut mask: u64 = 0;
                for _ in 0..NUMBER_OF_RANGES {
                    let range_start = unwrap_ok!(p2ra.allocate_range(range_length));
                    assert_lt!(range_start, total_size);
                    let bit = 1u64 << (range_start / range_length);
                    assert_eq!(mask & bit, 0u64);
                    mask |= bit;
                }
                for idx in 0..NUMBER_OF_RANGES {
                    p2ra.free_range(range_length * idx, range_length);
                }
                p2ra.free();
            }
        }

        {
            // We should be able to allocate an entire range, free a hole, and
            // reallocate in the same place.
            for range_length in [1u32, 4, 16] {
                const NUMBER_OF_RANGES: u32 = 64;
                let total_size = NUMBER_OF_RANGES * range_length;
                stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
                assert_ok!(p2ra.init(total_size));
                assert_ok!(p2ra.add_range(0, total_size));
                let mut mask: u64 = 0;
                for _ in 0..NUMBER_OF_RANGES {
                    let range_start = unwrap_ok!(p2ra.allocate_range(range_length));
                    assert_lt!(range_start, total_size);
                    let bit = 1u64 << (range_start / range_length);
                    assert_eq!(mask & bit, 0u64);
                    mask |= bit;
                }
                // Actually make and refill the holes.
                for idx in 0..NUMBER_OF_RANGES {
                    p2ra.free_range(range_length * idx, range_length);
                    let range_start = unwrap_ok!(p2ra.allocate_range(range_length));
                    assert_eq!(range_start, idx * range_length);
                }
                // Clean up.
                for idx in 0..NUMBER_OF_RANGES {
                    p2ra.free_range(range_length * idx, range_length);
                }
                p2ra.free();
            }
        }

        {
            // We should be able to allocate an entire range, free some
            // contiguous small holes, and reallocate larger ranges in the
            // same place.
            for range_length in [2u32, 4, 8] {
                for ranges_per_large_range in [2u32, 4, 8] {
                    let large_range_length = ranges_per_large_range * range_length;
                    const NUMBER_OF_RANGES: u32 = 64;
                    let number_of_large_ranges = NUMBER_OF_RANGES / ranges_per_large_range;
                    let total_size = NUMBER_OF_RANGES * range_length;
                    stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
                    assert_ok!(p2ra.init(total_size));
                    assert_ok!(p2ra.add_range(0, total_size));
                    let mut mask: u64 = 0;
                    for _ in 0..NUMBER_OF_RANGES {
                        let range_start = unwrap_ok!(p2ra.allocate_range(range_length));
                        assert_lt!(range_start, total_size);
                        let bit = 1u64 << (range_start / range_length);
                        assert_eq!(mask & bit, 0u64);
                        mask |= bit;
                    }
                    // Actually make and refill the holes.
                    for idx in 0..number_of_large_ranges {
                        for subidx in 0..ranges_per_large_range {
                            let range_start =
                                ((idx * ranges_per_large_range) + subidx) * range_length;
                            p2ra.free_range(range_start, range_length);
                        }
                        let large_range_start = unwrap_ok!(p2ra.allocate_range(large_range_length));
                        assert_eq!(large_range_start, idx * large_range_length);
                    }
                    // Clean up.
                    for idx in 0..number_of_large_ranges {
                        p2ra.free_range(large_range_length * idx, large_range_length);
                    }
                    p2ra.free();
                }
            }
        }

        {
            // Fragmentation should be able to prevent us from allocating.
            for range_length in [1u32, 4, 16] {
                const NUMBER_OF_RANGES: u32 = core::mem::size_of::<u64>() as u32;
                let total_size = NUMBER_OF_RANGES * range_length;
                const STRIDE: u32 = 4;
                stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
                assert_ok!(p2ra.init(total_size));
                assert_ok!(p2ra.add_range(0, total_size));
                let mut mask: u64 = 0;
                for _ in 0..NUMBER_OF_RANGES {
                    let range_start = unwrap_ok!(p2ra.allocate_range(range_length));
                    assert_lt!(range_start, total_size);
                    let bit = 1u64 << (range_start / range_length);
                    assert_eq!(mask & bit, 0u64);
                    mask |= bit;
                }
                // Leave every 4th allocated, and free the rest.
                for idx in 0..NUMBER_OF_RANGES {
                    if idx % STRIDE == 0 {
                        continue;
                    }
                    p2ra.free_range(range_length * idx, range_length);
                }
                // It should now be impossible to allocate a 4-times larger range.
                assert_eq!(
                    Status::result_into_raw(p2ra.allocate_range(STRIDE * range_length).map(|_| ())),
                    Status::NO_RESOURCES.into_raw()
                );
                // Clean up the remaining gaps.
                let mut idx = 0;
                while idx < NUMBER_OF_RANGES {
                    p2ra.free_range(range_length * idx, range_length);
                    idx += STRIDE;
                }
                p2ra.free();
            }
        }

        {
            // If we initialize a small size, and then add a larger range, we
            // should be able to spread out over the larger range.
            for range_length in [1u32, 4, 16] {
                // This time, the maximum size of an allocation is less than the
                // full space we will add.
                const SPARSENESS: u32 = 2;
                const NUMBER_OF_RANGES: u32 = 64 / SPARSENESS;
                let total_size = NUMBER_OF_RANGES * range_length;
                let upper_bound = 2 * total_size;
                stack_pin_init!(let p2ra = Pow2RangeAllocator::new());
                assert_ok!(p2ra.init(total_size));
                // The range is larger than the initialized size
                assert_ok!(p2ra.add_range(0, 2 * total_size));
                // Allocate as much as we can.
                let mut mask: u64 = 0;
                // Track in particular if any of our ranges are outside [0, total_size).
                let mut got_up_high = false;
                for _ in 0..NUMBER_OF_RANGES {
                    let range_start = unwrap_ok!(p2ra.allocate_range(range_length));
                    // Note that the upper bound here is bigger, by design.
                    assert_lt!(range_start, upper_bound);
                    let bit = 1u64 << (range_start / range_length);
                    assert_eq!(mask & bit, 0u64);
                    mask |= bit;
                    if range_start >= total_size {
                        got_up_high = true;
                    }
                }
                // If we already set some high ranges, we've proved our
                // point. Otherwise, we only have a pile of contiguous
                // ranges. So can free any two non-contiguous ranges, and
                // allocate a slightly bigger one. That slightly bigger one will
                // be forced to fit higher up.
                if !got_up_high {
                    // Double check our logic. If we never got allocated a high range, then mask
                    // better be all low bits.
                    assert_eq!(mask, 0xffffffffu64);
                    // Free a non-contiguous pair of small ranges (at spots 0 and 2).
                    p2ra.free_range(0, range_length);
                    p2ra.free_range(2 * range_length, range_length);
                    // Now we should be allocate a range twice as big.
                    let range_start = unwrap_ok!(p2ra.allocate_range(2 * range_length));
                    // And it must be somewhere after |total_size|.
                    assert_ge!(range_start, total_size);
                    // Let the big one go now.
                    p2ra.free_range(range_start, 2 * range_length);
                }
                // Clean up.
                for idx in 0..NUMBER_OF_RANGES {
                    if !got_up_high && (idx == 0 || idx == 2) {
                        // We freed these just above, already.
                        continue;
                    }
                    p2ra.free_range(range_length * idx, range_length);
                }
                p2ra.free();
            }
        }
    }
}
