// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/spsc_buffer/spsc_buffer.h>
#include <lib/unittest/unittest.h>

#include <fbl/alloc_checker.h>
#include <ktl/array.h>
#include <ktl/byte.h>
#include <ktl/span.h>

// The SpscBufferTests class is a friend of the SpscBuffer class, thus allowing tests to access
// private members of that class.
class SpscBufferTests {
 public:
  // Test that splitting combined pointers into the constituent read and write pointers works.
  static bool TestSplitCombinedPointers() {
    BEGIN_TEST;

    struct TestCase {
      uint64_t combined_pointers;
      SpscBuffer<HeapAllocator>::RingPointers expected;
    };
    constexpr ktl::array kTestCases = ktl::to_array<TestCase>({
        // Basic case.
        {
            .combined_pointers = 0x1320087'01005382,
            .expected = {.read = 0x1320087, .write = 0x1005382},
        },
        // Edge case where the pointers are 0.
        {
            .combined_pointers = 0,
            .expected = {.read = 0, .write = 0},
        },
        // Edge case where the highest bit of the read pointer is set.
        {
            .combined_pointers = 0xFFFFFFFF'00004587,
            .expected = {.read = 0xFFFFFFFF, .write = 0x4587},
        },
        // Edge case where the highest bit of the write pointer is set.
        {
            .combined_pointers = 0x4587'FFFFFFFF,
            .expected = {.read = 0x4587, .write = 0xFFFFFFFF},
        },
    });

    for (const TestCase& tc : kTestCases) {
      const SpscBuffer<HeapAllocator>::RingPointers actual =
          SpscBuffer<HeapAllocator>::SplitCombinedPointers(tc.combined_pointers);
      EXPECT_EQ(tc.expected.read, actual.read);
      EXPECT_EQ(tc.expected.write, actual.write);
    }

    END_TEST;
  }

  // Test that combining read and write pointers into a single combined pointer works.
  static bool TestCombinePointers() {
    BEGIN_TEST;

    struct TestCase {
      SpscBuffer<HeapAllocator>::RingPointers pointers;
      uint64_t combined;
    };
    constexpr ktl::array kTestCases = ktl::to_array<TestCase>({
        // Basic case.
        {
            .pointers = {.read = 0x1320087, .write = 0x1005382},
            .combined = 0x1320087'01005382,
        },
        // Edge case where the read and write pointers are 0.
        {
            .pointers = {.read = 0, .write = 0},
            .combined = 0,
        },
        // Edge case where the highest bit of the read pointer is set.
        {
            .pointers = {.read = 0xFFFFFFFF, .write = 0x123},
            .combined = 0xFFFFFFFF'00000123,
        },
        // Edge case where the highest bit of the write pointer is set.
        {
            .pointers = {.read = 0x123, .write = 0xFFFFFFFF},
            .combined = 0x123'FFFFFFFF,
        },
    });

    for (const TestCase& tc : kTestCases) {
      const uint64_t actual = SpscBuffer<HeapAllocator>::CombinePointers(tc.pointers);
      EXPECT_EQ(tc.combined, actual);
    }

    END_TEST;
  }

  // Test that the amount of available space in the ring buffer is correctly calculated.
  static bool TestAvailableSpace() {
    BEGIN_TEST;

    struct TestCase {
      SpscBuffer<HeapAllocator>::RingPointers pointers;
      uint32_t buffer_size;
      uint32_t expected;
    };
    constexpr ktl::array kTestCases = ktl::to_array<TestCase>({
        // A completely uninitialized buffer should return an available space of 0.
        {
            .pointers = {.read = 0, .write = 0},
            .buffer_size = 0,
            .expected = 0,
        },
        // The next two cases verify that the available space is the size of the storage buffer when
        // the read and write pointers are equal.
        {
            .pointers = {.read = 0, .write = 0},
            .buffer_size = 16,
            .expected = 16,
        },
        {
            .pointers = {.read = 3, .write = 3},
            .buffer_size = 16,
            .expected = 16,
        },
        // Test that the available space is computed correctly when the write pointer is in front of
        // the read pointer.
        {
            .pointers = {.read = 3, .write = 7},
            .buffer_size = 16,
            .expected = 12,
        },
        // Test that the available space is computed correctly when the write pointer overflows and
        // wraps around.
        {
            .pointers = {.read = 0xFFFFFFFC, .write = 3},
            .buffer_size = 16,
            .expected = 9,
        },
        // Test that the available space is computed correctly when the buffer is full.
        {
            .pointers = {.read = 0, .write = 16},
            .buffer_size = 16,
            .expected = 0,
        },
    });

    for (const TestCase& tc : kTestCases) {
      // Create a buffer to test with.
      SpscBuffer<HeapAllocator> buffer;
      buffer.Init(tc.buffer_size);

      const uint32_t actual = buffer.AvailableSpace(tc.pointers);
      EXPECT_EQ(tc.expected, actual);
    }

    END_TEST;
  }

  // Test that the amount of available data in the ring buffer is correctly calculated.
  static bool TestAvailableData() {
    BEGIN_TEST;

    struct TestCase {
      SpscBuffer<HeapAllocator>::RingPointers pointers;
      size_t buffer_size;
      uint32_t expected;
    };
    constexpr ktl::array kTestCases = ktl::to_array<TestCase>({
        // A completely uninitialized buffer should return an available data of 0.
        {
            .pointers = {.read = 0, .write = 0},
            .buffer_size = 0,
            .expected = 0,
        },
        // The next two cases verify that the available data is 0 when the read and write pointers
        // are equal.
        {
            .pointers = {.read = 0, .write = 0},
            .buffer_size = 16,
            .expected = 0,
        },
        {
            .pointers = {.read = 3, .write = 3},
            .buffer_size = 16,
            .expected = 0,
        },
        // Test that the available data is computed correctly when the write pointer is in front of
        // the read pointer.
        {
            .pointers = {.read = 3, .write = 7},
            .buffer_size = 16,
            .expected = 4,
        },
        // Test that the available data is computed correctly when the buffer is full.
        {
            .pointers = {.read = 0, .write = 16},
            .buffer_size = 16,
            .expected = 16,
        },
        // Test that the available data is computed correctly when the write pointer overflows and
        // wraps around.
        {
            .pointers = {.read = 0xFFFFFFFC, .write = 0x3},
            .buffer_size = 16,
            .expected = 7,
        },
    });

    for (const TestCase& tc : kTestCases) {
      const uint32_t actual = SpscBuffer<HeapAllocator>::AvailableData(tc.pointers);
      EXPECT_EQ(tc.expected, actual);
    }

    END_TEST;
  }

  // Test that advancing the read pointer works as expected.
  static bool TestAdvanceReadPointer() {
    BEGIN_TEST;

    struct TestCase {
      SpscBuffer<HeapAllocator>::RingPointers initial_pointers;
      uint32_t buffer_size;
      uint32_t delta;
      uint64_t expected;
    };

    constexpr ktl::array kTestCases = ktl::to_array<TestCase>({
        // Basic case.
        {
            .initial_pointers = {.read = 1, .write = 6},
            .buffer_size = 16,
            .delta = 4,
            .expected = 0x5'00000006,
        },
        // Test that the read pointer can be equal to the size of the buffer.
        {
            .initial_pointers = {.read = 0, .write = 16},
            .buffer_size = 16,
            .delta = 16,
            .expected = 0x10'00000010,
        },
        // Test that the read pointer wrapping works correctly.
        {
            .initial_pointers = {.read = 0xFFFFFFFC, .write = 12},
            .buffer_size = 16,
            .delta = 5,
            .expected = 0x1'0000000C,
        },
    });

    for (const TestCase& tc : kTestCases) {
      // Create a buffer to test with.
      SpscBuffer<HeapAllocator> buffer;
      buffer.Init(tc.buffer_size);

      // Set the initial value of the combined pointers.
      const uint64_t initial = SpscBuffer<HeapAllocator>::CombinePointers(tc.initial_pointers);
      buffer.combined_pointers_.store(initial, ktl::memory_order_release);

      // Advance the read pointer and ensure that combined_pointers_ is correct.
      buffer.AdvanceReadPointer(tc.initial_pointers, tc.delta);
      const uint64_t actual = buffer.combined_pointers_.load(ktl::memory_order_acquire);
      EXPECT_EQ(tc.expected, actual);
    }

    END_TEST;
  }

  // Test that advancing the write pointer works as expected.
  static bool TestAdvanceWritePointer() {
    BEGIN_TEST;

    struct TestCase {
      SpscBuffer<HeapAllocator>::RingPointers initial_pointers;
      uint32_t buffer_size;
      uint32_t delta;
      uint64_t expected;
    };

    constexpr ktl::array kTestCases = ktl::to_array<TestCase>({
        // Test that the read pointer is preserved.
        {
            .initial_pointers = {.read = 7, .write = 9},
            .buffer_size = 16,
            .delta = 4,
            .expected = 0x7'0000000D,
        },
        // Test that the write pointer can be equal to the size of the buffer.
        {
            .initial_pointers = {.read = 0, .write = 0},
            .buffer_size = 16,
            .delta = 16,
            .expected = 0x10,
        },
        // Test that the write pointer wrapping works correctly.
        {
            .initial_pointers = {.read = 12, .write = 0xFFFFFFFC},
            .buffer_size = 16,
            .delta = 5,
            .expected = 0xC'00000001,
        },
    });

    for (const TestCase& tc : kTestCases) {
      // Create a buffer to test with.
      SpscBuffer<HeapAllocator> buffer;
      buffer.Init(tc.buffer_size);

      // Set the initial value of the combined pointers.
      const uint64_t initial = SpscBuffer<HeapAllocator>::CombinePointers(tc.initial_pointers);
      buffer.combined_pointers_.store(initial, ktl::memory_order_release);

      // Advance the write pointer and ensure that combined_pointers_ is correct.
      buffer.AdvanceWritePointer(tc.initial_pointers, tc.delta);
      const uint64_t actual = buffer.combined_pointers_.load(ktl::memory_order_acquire);
      EXPECT_EQ(tc.expected, actual);
    }

    END_TEST;
  }

  // Test that initialization of the SpscBuffer works correctly.
  static bool TestInit() {
    BEGIN_TEST;

    // Happy case.
    {
      SpscBuffer<HeapAllocator> spsc;
      ASSERT_OK(spsc.Init(256));
    }

    // Calling Init with too big of a size should fail.
    {
      SpscBuffer<HeapAllocator> spsc;
      ASSERT_EQ(ZX_ERR_INVALID_ARGS, spsc.Init(UINT32_MAX));
    }

    // Calling Init with a size that is not a power of two should fail.
    {
      SpscBuffer<HeapAllocator> spsc;
      ASSERT_EQ(ZX_ERR_INVALID_ARGS, spsc.Init(100));
    }

    // Calling Init on an already initialized buffer should fail.
    {
      SpscBuffer<HeapAllocator> spsc;
      ASSERT_OK(spsc.Init(256));
      ASSERT_EQ(ZX_ERR_BAD_STATE, spsc.Init(256));
    }

    // Init should propagate allocation failures.
    {
      SpscBuffer<ErrorAllocator> spsc;
      ASSERT_EQ(ZX_ERR_NO_MEMORY, spsc.Init(256));
    }

    END_TEST;
  }

  // Test that reads and writes from the same thread work correctly.
  static bool TestReadWriteSingleThreaded() {
    BEGIN_TEST;

    // Start by setting up a:
    // * Storage buffer, which will be the backing storage for the ring buffer.
    // * Src buffer, which will be filled with random data that will be written to the ring buffer.
    // * Dst buffer, which will be used to read data out of the ring buffer.
    constexpr size_t kStorageSize = 256;
    ktl::array<ktl::byte, kStorageSize> src;
    srand(4);
    for (ktl::byte& byte : src) {
      byte = static_cast<ktl::byte>(rand());
    }
    ktl::array<ktl::byte, kStorageSize> dst;

    // Set up the lambda that the read method will use to copy data into the destination buffer.
    auto copy_out_fn = [&dst](uint32_t offset, ktl::span<ktl::byte> src) {
      // The given offset and source buffer should be able to fit inside the destination.
      ASSERT((offset + src.size()) <= dst.size());
      memcpy(dst.data() + offset, src.data(), src.size());
      return ZX_OK;
    };

    // Set up a copy out function that returns an error.
    auto copy_out_err_fn = [](uint32_t offset, ktl::span<ktl::byte> src) {
      return ZX_ERR_BAD_STATE;
    };

    // Set up our test cases.
    struct TestCase {
      uint32_t write_size;
      uint32_t read_size;
      uint32_t expected_read_size;
      zx_status_t expected_reserve_status;
      zx_status_t expected_read_status;
      SpscBuffer<HeapAllocator>::RingPointers initial_pointers;
      bool use_copy_out_err_fn;
    };
    constexpr ktl::array kTestCases = ktl::to_array<TestCase>({
        // Test the case with no wrapping and we read everything we wrote.
        {
            .write_size = kStorageSize / 2,
            .read_size = kStorageSize / 2,
            .expected_read_size = kStorageSize / 2,
            .expected_reserve_status = ZX_OK,
            .expected_read_status = ZX_OK,
            .initial_pointers = {.read = 0, .write = 0},
            .use_copy_out_err_fn = false,
        },
        // Test the case with no wrapping and we perform a partial read of what we wrote.
        {
            .write_size = kStorageSize / 2,
            .read_size = kStorageSize / 4,
            .expected_read_size = kStorageSize / 4,
            .expected_reserve_status = ZX_OK,
            .expected_read_status = ZX_OK,
            .initial_pointers = {.read = 0, .write = 0},
            .use_copy_out_err_fn = false,
        },
        // Test the case where we fill the buffer and read the whole thing back out.
        {
            .write_size = kStorageSize,
            .read_size = kStorageSize,
            .expected_read_size = kStorageSize,
            .expected_reserve_status = ZX_OK,
            .expected_read_status = ZX_OK,
            .initial_pointers = {.read = 0, .write = 0},
            .use_copy_out_err_fn = false,
        },
        // Test the case where we attempt to read more data than we wrote.
        {
            .write_size = kStorageSize / 4,
            .read_size = kStorageSize / 2,
            .expected_read_size = kStorageSize / 4,
            .expected_reserve_status = ZX_OK,
            .expected_read_status = ZX_OK,
            .initial_pointers = {.read = 0, .write = 0},
            .use_copy_out_err_fn = false,
        },
        // Test the case where a read and write both wrap around the end of the ring buffer.
        {
            .write_size = kStorageSize,
            .read_size = kStorageSize,
            .expected_read_size = kStorageSize,
            .expected_reserve_status = ZX_OK,
            .expected_read_status = ZX_OK,
            .initial_pointers = {.read = kStorageSize / 2, .write = kStorageSize / 2},
            .use_copy_out_err_fn = false,
        },
        // Test the case where the read and write pointers wrap around the max uint32_t value.
        {
            .write_size = kStorageSize,
            .read_size = kStorageSize,
            .expected_read_size = kStorageSize,
            .expected_reserve_status = ZX_OK,
            .expected_read_status = ZX_OK,
            .initial_pointers = {.read = 0xFFFFFFFA, .write = 0xFFFFFFFA},
            .use_copy_out_err_fn = false,
        },
        // Test that writing more data than we have space for returns ZX_ERR_NO_SPACE.
        {
            .write_size = 64,
            .expected_reserve_status = ZX_ERR_NO_SPACE,
            .initial_pointers = {.read = 0, .write = kStorageSize - 48},
        },
        // Test that the copy out function returning an error does not read any data.
        {
            .write_size = kStorageSize,
            .read_size = kStorageSize / 2,
            .expected_read_status = ZX_ERR_BAD_STATE,
            .initial_pointers = {.read = 0, .write = 0},
            .use_copy_out_err_fn = true,
        },
    });

    for (const TestCase& tc : kTestCases) {
      // Zero out the destination buffer to remove any stale data from previous test cases.
      memset(dst.data(), 0, kStorageSize);

      // Initialize a SpscBuffer to run tests with.
      SpscBuffer<HeapAllocator> spsc;
      ASSERT_OK(spsc.Init(kStorageSize));

      // Set the initial value of the starting pointers.
      const uint64_t starting_pointers =
          SpscBuffer<HeapAllocator>::CombinePointers(tc.initial_pointers);
      spsc.combined_pointers_.store(starting_pointers, ktl::memory_order_release);

      // Reserve a slot of the requested size in the ring buffer and validate that we get the
      // expected return code.
      zx::result<SpscBuffer<HeapAllocator>::Reservation> reservation = spsc.Reserve(tc.write_size);
      ASSERT_EQ(tc.expected_reserve_status, reservation.status_value());
      if (tc.expected_reserve_status != ZX_OK) {
        continue;
      }

      // Perform the write and commit.
      reservation->Write(ktl::span<ktl::byte>(src.data(), tc.write_size));
      reservation->Commit();

      // Perform the read using the desired copy out function and validate that we get the expected
      // return code.
      const zx::result<size_t> read_result = [&]() {
        if (tc.use_copy_out_err_fn) {
          return spsc.Read(copy_out_err_fn, tc.read_size);
        }
        return spsc.Read(copy_out_fn, tc.read_size);
      }();
      ASSERT_EQ(tc.expected_read_status, read_result.status_value());
      if (tc.expected_read_status != ZX_OK) {
        // If the read failed, then the read pointer should not have advanced, and there should be
        // just as much data after the read call as there was before, so assert that here.
        ASSERT_EQ(spsc.AvailableData(spsc.LoadPointers()), tc.write_size);
        continue;
      }

      // Verify that we read out the correct data.
      ASSERT_EQ(tc.expected_read_size, read_result.value());
      ASSERT_BYTES_EQ(reinterpret_cast<uint8_t*>(src.data()),
                      reinterpret_cast<uint8_t*>(dst.data()), tc.expected_read_size);
    }

    END_TEST;
  }

  // Test that draining the buffer works.
  static bool TestDrain() {
    BEGIN_TEST;

    // Initialize the backing storage for the SPSC buffer.
    constexpr uint32_t kStorageSize = 256;
    ktl::array<ktl::byte, kStorageSize> storage;

    // Initialize the SPSC buffer and write some data into it.
    // This write is done by setting bytes in the storage buffer directly and then modifying the
    // read and write pointers for simplicity.
    SpscBuffer<HeapAllocator> spsc;
    ASSERT_OK(spsc.Init(kStorageSize));
    memset(storage.data(), 'f', kStorageSize / 2);
    const uint64_t starting_pointers = SpscBuffer<HeapAllocator>::CombinePointers({
        .read = 0,
        .write = kStorageSize / 2,
    });
    spsc.combined_pointers_.store(starting_pointers, ktl::memory_order_release);

    // Drain the buffer, then verify that it is empty.
    spsc.Drain();
    ASSERT_EQ(0u, spsc.AvailableData(spsc.LoadPointers()));

    END_TEST;
  }

 private:
  // Allocator used by the TestInit function to validate proper error behavior when a nullptr
  // is returned by Allocate.
  class ErrorAllocator {
   public:
    static ktl::byte* Allocate(uint32_t size) { return nullptr; }
    static void Free(ktl::byte* ptr) { DEBUG_ASSERT(ptr == nullptr); }
  };

  // Traditional HeapAllocator to use in the rest of the tests.
  class HeapAllocator {
   public:
    static ktl::byte* Allocate(uint32_t size) {
      fbl::AllocChecker ac;
      ktl::byte* ptr = new (&ac) ktl::byte[size];
      if (!ac.check()) {
        return nullptr;
      }
      return ptr;
    }
    static void Free(ktl::byte* ptr) { delete[] ptr; }
  };
};

UNITTEST_START_TESTCASE(spsc_buffer_tests)
UNITTEST("split_pointers", SpscBufferTests::TestSplitCombinedPointers)
UNITTEST("combine_pointers", SpscBufferTests::TestCombinePointers)
UNITTEST("available_space", SpscBufferTests::TestAvailableSpace)
UNITTEST("available_data", SpscBufferTests::TestAvailableData)
UNITTEST("advance_read_pointer", SpscBufferTests::TestAdvanceReadPointer)
UNITTEST("advance_write_pointer", SpscBufferTests::TestAdvanceWritePointer)
UNITTEST("init", SpscBufferTests::TestInit)
UNITTEST("read_write_single_threaded", SpscBufferTests::TestReadWriteSingleThreaded)
UNITTEST("drain", SpscBufferTests::TestDrain)
UNITTEST_END_TESTCASE(spsc_buffer_tests, "spsc_buffer",
                      "Test the single-producer, single-consumer ring buffer implementation.")
