// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/memalloc/pool.h>
#include <lib/memalloc/range.h>
#include <lib/memalloc/testing/range.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <array>
#include <limits>
#include <optional>
#include <span>
#include <vector>

#include <gtest/gtest.h>

#include "test.h"

namespace {

using namespace std::string_view_literals;

using memalloc::Pool;
using memalloc::Range;
using memalloc::Type;

constexpr uint64_t kChunkSize = Pool::kBookkeepingChunkSize;
constexpr uint64_t kDefaultAlignment = __STDCPP_DEFAULT_NEW_ALIGNMENT__;
constexpr uint64_t kDefaultMinAddr = 0;
constexpr uint64_t kDefaultMaxAddr = std::numeric_limits<uintptr_t>::max();
constexpr uint64_t kUint64Max = std::numeric_limits<uint64_t>::max();

constexpr const char* kPrintOutPrefix = "PREFIX";

constexpr std::string_view kEmptyPrintOut =
    R"""(PREFIX: | Physical memory range                    | Size    | Type
)""";

void TestPoolInit(Pool& pool, std::span<Range> input, std::optional<uint64_t> min_addr = {},
                  std::optional<uint64_t> max_addr = {}, bool init_error = false) {
  auto status = pool.Init(std::array{input}, min_addr.value_or(kDefaultMinAddr),
                          max_addr.value_or(kDefaultMaxAddr));
  if (init_error) {
    ASSERT_TRUE(status.is_error());
    return;
  }
  ASSERT_FALSE(status.is_error());
}

void TestPoolContents(const Pool& pool, std::span<const Range> expected) {
  EXPECT_EQ(expected.size(), pool.size());
  std::vector<Range> actual(pool.begin(), pool.end());
  ASSERT_NO_FATAL_FAILURE(memalloc::testing::CompareRanges(expected, {actual}));
}

void TestPoolPrintOut(const Pool& pool, const char* prefix, std::string_view expected) {
  constexpr size_t kPrintOutSizeMax = 0x400;

  FILE* f = tmpfile();
  auto cleanup = fit::defer([f]() { fclose(f); });

  pool.PrintMemoryRanges(prefix, f);
  rewind(f);

  char buff[kPrintOutSizeMax];
  size_t n = fread(buff, 1, kPrintOutSizeMax, f);
  ASSERT_EQ(0, ferror(f)) << "failed to read file: " << strerror(errno);

  std::string_view actual{buff, n};
  EXPECT_EQ(expected, actual);
}

void TestPoolAllocation(Pool& pool, Type type, uint64_t size, uint64_t alignment,
                        std::optional<uint64_t> min_addr = {},
                        std::optional<uint64_t> max_addr = {}, bool alloc_error = false) {
  auto result = pool.Allocate(type, size, alignment, min_addr, max_addr);
  if (alloc_error) {
    ASSERT_TRUE(result.is_error());
    return;
  }
  ASSERT_FALSE(result.is_error());

  // The resulting range should now be contained in one of the tracked ranges.
  const Range contained = {
      .addr = std::move(result).value(),
      .size = size,
      .type = type,
  };
  auto is_contained = [&pool](const Range& subrange) -> bool {
    for (const Range& range : pool) {
      if (range.addr <= subrange.addr && subrange.end() <= range.end()) {
        return range.type == subrange.type;
      }
    }
    return false;
  };
  EXPECT_TRUE(is_contained(contained));
}

void TestPoolFreeing(Pool& pool, uint64_t addr, uint64_t size, bool free_error = false) {
  // Returns the tracked type of a single address, if any.
  //
  // Asserting that a range is contained within the union of a connected set
  // of subranges is a bit complicated; accordingly, so assert below on the
  // weaker proposition that inclusive endpoints are tracked (and with expected
  // types).
  auto tracked_type = [&pool](uint64_t addr) -> std::optional<Type> {
    for (const Range& range : pool) {
      if (range.addr <= addr && addr < range.end()) {
        return range.type;
      }
    }
    return std::nullopt;
  };

  EXPECT_TRUE(tracked_type(addr));
  if (size) {
    EXPECT_TRUE(tracked_type(addr + size - 1));
  }

  auto result = pool.Free(addr, size);
  if (free_error) {
    ASSERT_TRUE(result.is_error());
    return;
  }
  ASSERT_FALSE(result.is_error());

  EXPECT_EQ(Type::kFreeRam, tracked_type(addr));
  if (size) {
    EXPECT_EQ(Type::kFreeRam, tracked_type(addr + size - 1));
  }
}

void TestPoolRamSubrangeUpdating(Pool& pool, Type type, uint64_t addr, uint64_t size,
                                 bool alloc_error = false) {
  auto status = pool.UpdateRamSubranges(type, addr, size);
  if (alloc_error) {
    EXPECT_TRUE(status.is_error());
    return;
  }
  EXPECT_FALSE(status.is_error());
}

void TestPoolResizing(Pool& pool, const Range& original, uint64_t new_size, uint64_t min_alignment,
                      std::optional<uint64_t> expected) {
  auto status = pool.Resize(original, new_size, min_alignment);
  if (!expected) {
    EXPECT_TRUE(status.is_error());
    return;
  }
  ASSERT_FALSE(status.is_error());
  EXPECT_EQ(*expected, status.value());
}

// Fills up a pool with two-byte allocations of varying types until its
// bookkeeping space is used up.
void Oom(Pool& pool) {
  // Start just after kPoolBookkeeping, to ensure we don't try to allocate a bad
  // type.
  uint64_t type_val = static_cast<uint64_t>(Type::kPoolBookkeeping) + 1;
  bool failure = false;
  while (!failure && type_val < kUint64Max) {
    Type type = static_cast<Type>(type_val++);
    failure = pool.Allocate(type, 2, 1).is_error();
  }
  ASSERT_NE(type_val, kUint64Max);  // This should never happen.
}

TEST(MemallocPoolTests, NoInputMemory) {
  PoolContext ctx;

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {}, {}, {}, /*init_error=*/true));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {}));
  ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kEmptyPrintOut));
}

TEST(MemallocPoolTests, NoRam) {
  PoolContext ctx;
  Range ranges[] = {
      // reserved: [0, kChunkSize)
      {
          .addr = 0,
          .size = kChunkSize,
          .type = Type::kReserved,
      },
      // peripheral: [kChunkSize, 2*kChunkSize)
      {
          .addr = kChunkSize,
          .size = kChunkSize,
          .type = Type::kPeripheral,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}, {}, {}, /*init_error=*/true));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {}));
  ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kEmptyPrintOut));
}

TEST(MemallocPoolTests, TooLittleRam) {
  {
    PoolContext ctx;
    Range ranges[] = {
        // RAM: [0, kChunkSize - 1)
        {
            .addr = 0,
            .size = kChunkSize - 1,
            .type = Type::kFreeRam,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}, {}, {}, /*init_error=*/true));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {}));
    ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kEmptyPrintOut));
  }

  {
    PoolContext ctx;
    Range ranges[] = {
        // reserved: [0, kChunkSize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kReserved,
        },
        // RAM: [kChunkSize, kChunkSize/2)
        {
            .addr = kChunkSize,
            .size = kChunkSize / 2,
            .type = Type::kFreeRam,
        },
        // reserved: [kChunkSize/2, 3*kChunkSize/4)
        {
            .addr = kChunkSize / 2,
            .size = kChunkSize / 4,
            .type = Type::kReserved,
        },
        // RAM: [3*kChunkSize/4, 7*kChunkSize/8)
        {
            .addr = 3 * kChunkSize / 4,
            .size = kChunkSize / 8,
            .type = Type::kFreeRam,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}, {}, {}, /*init_error=*/true));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {}));
    ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kEmptyPrintOut));
  }
}

TEST(MemallocPoolTests, Bookkeeping) {
  {
    PoolContext ctx;
    Range ranges[] = {
        // RAM: [0, kChunkSize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    const Range expected[] = {
        // bookkeeping: [0, kChunkSize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
    };

    constexpr std::string_view kExpectedPrintOut =
        R"""(PREFIX: | Physical memory range                    | Size    | Type
PREFIX: | [0x0000000000000000, 0x0000000000001000) |      4K | bookkeeping
)""";

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
    ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kExpectedPrintOut));
  }

  {
    PoolContext ctx;
    Range ranges[] = {
        // RAM: [0, kChunkSize)
        {
            .addr = 0,
            .size = 2 * kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    const Range expected[] = {
        // bookkeeping: [0, kChunkSize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        // RAM: [kChunkSize, 2*kChunkSize)
        {
            .addr = kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    constexpr std::string_view kExpectedPrintOut =
        R"""(PREFIX: | Physical memory range                    | Size    | Type
PREFIX: | [0x0000000000000000, 0x0000000000001000) |      4K | bookkeeping
PREFIX: | [0x0000000000001000, 0x0000000000002000) |      4K | free RAM
)""";

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
    ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kExpectedPrintOut));
  }

  {
    PoolContext ctx;
    Range ranges[] = {
        // peripheral: [0, kChunkSize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPeripheral,
        },
        // RAM: [kChunkSize, 2*kChunkSize)
        {
            .addr = kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    const Range expected[] = {
        // peripheral: [0, kChunkSize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPeripheral,
        },
        // bookkeeping: [kChunkSize, 2*kChunkSize)
        {
            .addr = kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
    };

    constexpr std::string_view kExpectedPrintOut =
        R"""(PREFIX: | Physical memory range                    | Size    | Type
PREFIX: | [0x0000000000000000, 0x0000000000001000) |      4K | peripheral
PREFIX: | [0x0000000000001000, 0x0000000000002000) |      4K | bookkeeping
)""";

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
    ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kExpectedPrintOut));
  }

  {
    PoolContext ctx;
    Range ranges[] = {
        // RAM: [kChunkSize/2, 2*kChunkSize)
        {
            .addr = kChunkSize / 2,
            .size = 3 * kChunkSize / 2,
            .type = Type::kFreeRam,
        },
    };

    const Range expected[] = {
        // RAM: [kChunkSize/2, kChunkSize)
        {
            .addr = kChunkSize / 2,
            .size = kChunkSize / 2,
            .type = Type::kFreeRam,
        },
        // bookkeeping: [kChunkSize, 2*kChunkSize)
        {
            .addr = kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
    };

    constexpr std::string_view kExpectedPrintOut =
        R"""(PREFIX: | Physical memory range                    | Size    | Type
PREFIX: | [0x0000000000000800, 0x0000000000001000) |      2K | free RAM
PREFIX: | [0x0000000000001000, 0x0000000000002000) |      4K | bookkeeping
)""";

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
    ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kExpectedPrintOut));
  }

  {
    PoolContext ctx;
    Range ranges[] = {
        // RAM: [0, 2*kChunkSize)
        {
            .addr = 0,
            .size = 2 * kChunkSize,
            .type = Type::kFreeRam,
        },
        // peripheral: [kChunkSize/2, 3*kChunkSize/2)
        {
            .addr = kChunkSize / 2,
            .size = kChunkSize,
            .type = Type::kPeripheral,
        },
        // RAM: [2*kChunkSize, 3*kChunkSize)
        {
            .addr = 2 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    const Range expected[] = {
        // RAM: [0. kChunkSize/2)
        {
            .addr = 0,
            .size = kChunkSize / 2,
            .type = Type::kFreeRam,
        },
        // peripheral: [kChunkSize/2, 3*kChunkSize/2)
        {
            .addr = kChunkSize / 2,
            .size = kChunkSize,
            .type = Type::kPeripheral,
        },
        // RAM: [3*kChunkSize/2. 2*kChunkSize)
        {
            .addr = 3 * kChunkSize / 2,
            .size = kChunkSize / 2,
            .type = Type::kFreeRam,
        },
        // bookkeeping: [2*kChunkSize, 3*kChunkSize)
        {
            .addr = 2 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
    };

    constexpr std::string_view kExpectedPrintOut =
        R"""(PREFIX: | Physical memory range                    | Size    | Type
PREFIX: | [0x0000000000000000, 0x0000000000000800) |      2K | free RAM
PREFIX: | [0x0000000000000800, 0x0000000000001800) |      4K | peripheral
PREFIX: | [0x0000000000001800, 0x0000000000002000) |      2K | free RAM
PREFIX: | [0x0000000000002000, 0x0000000000003000) |      4K | bookkeeping
)""";

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
    ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kExpectedPrintOut));
  }
}

TEST(MemallocPoolTests, ReservedRangesAreNotExplicitlyTracked) {
  {
    PoolContext ctx;
    Range ranges[] = {
        // reserved: [0, kChunkSize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kReserved,
        },
        // free RAM: [0, 3 * kChunkSize)
        {
            .addr = 0,
            .size = 3 * kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    const Range expected[] = {
        // bookkeeping: [kChunksize,  2 * kChunkSize)
        {
            .addr = kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        // RAM: [2 * kChunksize,  3 * kChunkSize)
        {
            .addr = 2 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    constexpr std::string_view kExpectedPrintOut =
        R"""(PREFIX: | Physical memory range                    | Size    | Type
PREFIX: | [0x0000000000001000, 0x0000000000002000) |      4K | bookkeeping
PREFIX: | [0x0000000000002000, 0x0000000000003000) |      4K | free RAM
)""";

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
    ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kExpectedPrintOut));
  }

  {
    PoolContext ctx;
    Range ranges[] = {
        // free RAM: [0, 3 * kChunkSize)
        {
            .addr = 0,
            .size = 3 * kChunkSize,
            .type = Type::kFreeRam,
        },
        // reserved: [kChunkSize, 2* kChunkSize)
        {
            .addr = kChunkSize,
            .size = kChunkSize,
            .type = Type::kReserved,
        },
    };

    const Range expected[] = {
        // bookkeeping: [0, kChunksize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        // RAM: [2 * kChunksize,  3 * kChunkSize)
        {
            .addr = 2 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    constexpr std::string_view kExpectedPrintOut =
        R"""(PREFIX: | Physical memory range                    | Size    | Type
PREFIX: | [0x0000000000000000, 0x0000000000001000) |      4K | bookkeeping
PREFIX: | [0x0000000000002000, 0x0000000000003000) |      4K | free RAM
)""";

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
    ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kExpectedPrintOut));
  }

  {
    PoolContext ctx;
    Range ranges[] = {
        // free RAM: [0, 3 * kChunkSize)
        {
            .addr = 0,
            .size = 3 * kChunkSize,
            .type = Type::kFreeRam,
        },
        // reserved: [2 * kChunkSize, 3 * kChunkSize)
        {
            .addr = 2 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kReserved,
        },
    };

    const Range expected[] = {
        // bookkeeping: [0, kChunksize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        // RAM: [kChunksize,  2 * kChunkSize)
        {
            .addr = kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    constexpr std::string_view kExpectedPrintOut =
        R"""(PREFIX: | Physical memory range                    | Size    | Type
PREFIX: | [0x0000000000000000, 0x0000000000001000) |      4K | bookkeeping
PREFIX: | [0x0000000000001000, 0x0000000000002000) |      4K | free RAM
)""";

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
    ASSERT_NO_FATAL_FAILURE(TestPoolPrintOut(ctx.pool, kPrintOutPrefix, kExpectedPrintOut));
  }
}

TEST(MemallocPoolTests, FindContainingRange) {
  PoolContext ctx;
  Range ranges[] = {
      // RAM: [0, 3*kChunkSize)
      {
          .addr = 0,
          .size = 3 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  const Range expected[] = {
      // bookkeeping: [0, kChunkSize)
      {
          .addr = 0,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      // RAM: [kChunkSize, 3*kChunkSize)
      {
          .addr = kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));

  EXPECT_EQ(expected[0], *ctx.pool.FindContainingRange(kDefaultMinAddr));
  EXPECT_EQ(expected[0], *ctx.pool.FindContainingRange(kChunkSize - 1));
  EXPECT_EQ(expected[1], *ctx.pool.FindContainingRange(kChunkSize));
  EXPECT_EQ(expected[1], *ctx.pool.FindContainingRange(2 * kChunkSize));
  EXPECT_EQ(expected[1], *ctx.pool.FindContainingRange(3 * kChunkSize - 1));
  EXPECT_EQ(ctx.pool.end(), ctx.pool.FindContainingRange(3 * kChunkSize));
}

TEST(MemallocPoolTests, DefaultAllocationBounds) {
  Range ranges[] = {
      // free RAM: [0, 100*kChunkSize)
      {
          .addr = 0,
          .size = 100 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  {
    // A sufficiently large minimum address leaves no room for bookkeeping.
    PoolContext ctx;
    ASSERT_NO_FATAL_FAILURE(
        TestPoolInit(ctx.pool, {ranges}, 100 * kChunkSize, 0, /*init_error=*/true));
  }

  {
    // A sufficiently small maximum address leaves no room for bookkeeping.
    PoolContext ctx;
    ASSERT_NO_FATAL_FAILURE(
        TestPoolInit(ctx.pool, {ranges}, 0, kChunkSize / 2, /*init_error=*/true));
  }

  {
    // Default bounds correspond to [10 * kChunkSize, 50 * kChunkSize).
    const Range after_init[] = {
        // free RAM: [0, 10*kChunkSize)
        {
            .addr = 0,
            .size = 10 * kChunkSize,
            .type = Type::kFreeRam,
        },
        // bookkeeping: [10*kChunkSize, 11*kChunkSize)
        {
            .addr = 10 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        // free RAM: [11*kChunkSize, 100*kChunkSize)
        {
            .addr = 11 * kChunkSize,
            .size = 89 * kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    PoolContext ctx;
    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}, 10 * kChunkSize, 50 * kChunkSize));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {after_init}));

    // Despite there being 89 chunks available past the bookkeeping, the pool's
    // default address bound means that only 39 of them are accessible unless
    // overridden.
    ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, 40 * kChunkSize,
                                               kDefaultAlignment, {}, {}, /*alloc_error=*/true));

    // Though we could override that default now to allocate the remaining chunks.
    ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, 89 * kChunkSize,
                                               kDefaultAlignment, {}, 100 * kChunkSize));

    // Similarly, despite there being 10 chunks before the bookkeeping, they
    // are inaccessible given the default lower bound, unless overridden.
    ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, 10 * kChunkSize,
                                               kDefaultAlignment, {}, {}, /*alloc_error=*/true));

    ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, 10 * kChunkSize,
                                               kDefaultAlignment, 0, {}));
  }
}

TEST(MemallocPoolTests, AccessCallback) {
  Range ranges[] = {
      // free RAM: [0, 100*kChunkSize)
      {
          .addr = 0,
          .size = 100 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  struct Accessed {
    uint64_t addr, size;
  };
  std::vector<Accessed> accessed;
  PoolContext ctx;
  ctx.pool.set_access_callback(
      [&accessed](uint64_t addr, uint64_t size) { accessed.push_back({addr, size}); });

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

  ASSERT_TRUE(!accessed.empty());
  EXPECT_EQ(1u, accessed.size());
  EXPECT_EQ(accessed[0].addr, 0u);
  EXPECT_EQ(accessed[0].size, kChunkSize);

  ASSERT_NO_FATAL_FAILURE(
      TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, 10 * kChunkSize, kDefaultAlignment));

  ASSERT_GT(accessed.size(), 1u);
  EXPECT_EQ(2u, accessed.size());
  EXPECT_EQ(accessed[1].addr, kChunkSize);
  EXPECT_EQ(accessed[1].size, 10 * kChunkSize);

  ctx.pool.set_access_callback([](uint64_t addr, uint64_t size) {});
  ASSERT_NO_FATAL_FAILURE(
      TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, 10 * kChunkSize, kDefaultAlignment));
  EXPECT_EQ(2u, accessed.size());
}

TEST(MemallocPoolTests, NoResourcesAllocation) {
  PoolContext ctx;
  Range ranges[] = {
      // free RAM: [kChunkSize, 3*kChunkSize)
      {
          .addr = kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  const Range expected[] = {
      // bookkeeping: [kChunkSize, 2*kChunkSize)
      {
          .addr = kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      // free RAM: [2*kChunkSize, 3*kChunkSize)
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));

  // Requested size is too big:
  ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, 2 * kChunkSize,
                                             kDefaultAlignment, {}, {},
                                             /*alloc_error=*/true));
  // Requested alignment is too big:
  ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, kChunkSize,
                                             kChunkSize << 2, {}, {},
                                             /*alloc_error=*/true));
  // Requested min address is too big:
  ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, kChunkSize,
                                             kDefaultAlignment, 2 * kChunkSize + 1, {},
                                             /*alloc_error=*/true));
  ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, kChunkSize,
                                             kDefaultAlignment, 3 * kChunkSize, {},
                                             /*alloc_error=*/true));

  // Requested max address is too small:
  ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, kChunkSize,
                                             kDefaultAlignment, {}, 3 * kChunkSize - 2,
                                             /*alloc_error=*/true));
  ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, kChunkSize,
                                             kDefaultAlignment, {}, 2 * kChunkSize,
                                             /*alloc_error=*/true));

  // Nothing should have changed.
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
}

TEST(MemallocPoolTests, AllocationWithEqualBounds) {
  Range ranges[] = {
      // free RAM: [kChunkSize, 3*kChunkSize)
      {
          .addr = kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  // Equal bounds passed to Init().
  {
    PoolContext ctx;
    ASSERT_NO_FATAL_FAILURE(
        TestPoolInit(ctx.pool, {ranges}, kChunkSize, kChunkSize, /*init_error=*/true));
  }

  // Equal bounds passed to Allocate().
  {
    const Range expected[] = {
        // bookkeeping: [kChunkSize, 2*kChunkSize)
        {
            .addr = kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        // free RAM: [2*kChunkSize, 3*kChunkSize)
        {
            .addr = 2 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    PoolContext ctx;
    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));

    // An allocation with size > 1 should fail.
    ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, 2,
                                               kDefaultAlignment, 2 * kChunkSize, 2 * kChunkSize,
                                               /*alloc_error=*/true));

    // But an allocation with size == 1 should succeed.
    ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, 1,
                                               kDefaultAlignment, 2 * kChunkSize, 2 * kChunkSize));
  }
}

TEST(MemallocPoolTests, ExhaustiveAllocation) {
  Range ranges[] = {
      // free RAM: [kChunkSize, 3*kChunkSize)
      {
          .addr = kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  const Range expected_before[] = {
      // bookkeeping: [kChunkSize, 2*kChunkSize)
      {
          .addr = kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      // free RAM: [2*kChunkSize, 3*kChunkSize)
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  const Range expected_after[] = {
      // bookkeeping: [kChunkSize, 2*kChunkSize)
      {
          .addr = kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      // free RAM: [2*kChunkSize, 3*kChunkSize)
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolTestPayload,
      },
  };

  {
    PoolContext ctx;

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_before}));

    ASSERT_NO_FATAL_FAILURE(
        TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, kChunkSize, kDefaultAlignment));

    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_after}));
  }

  {
    PoolContext ctx;

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_before}));

    for (size_t i = 0; i < 2; ++i) {
      ASSERT_NO_FATAL_FAILURE(
          TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, kChunkSize / 2, kDefaultAlignment));
    }

    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_after}));
  }

  {
    PoolContext ctx;

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_before}));

    for (size_t i = 0; i < 4; ++i) {
      ASSERT_NO_FATAL_FAILURE(
          TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, kChunkSize / 4, kDefaultAlignment));
    }

    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_after}));
  }

  {
    PoolContext ctx;

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_before}));

    for (size_t i = 0; i < kChunkSize; ++i) {
      ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, 1, 1));
    }

    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_after}));
  }
}

TEST(MemallocPoolTests, Freeing) {
  PoolContext ctx;
  Range ranges[] = {
      // RAM: [0, 2*kChunkSize)
      {
          .addr = 0,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
      // data ZBI: [2*kChunkSize, 3*kChunkSize)
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kDataZbi,
      },
  };

  const Range expected[] = {
      // bookkeeping: [0, kChunkSize)
      {
          .addr = 0,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      // free RAM: [kChunkSize, 2*kChunkSize)
      {
          .addr = kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
      // data ZBI: [2*kChunkSize, 3*kChunkSize)
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kDataZbi,
      },
  };

  const Range expected_after[] = {
      // bookkeeping: [0, kChunkSize)
      {
          .addr = 0,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      // free RAM: [kChunkSize, 3*kChunkSize)
      {
          .addr = kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));

  // A subrange of allocated type passed to Init() can be freed.
  ASSERT_NO_FATAL_FAILURE(TestPoolFreeing(ctx.pool, 2 * kChunkSize, kChunkSize / 2));
  ASSERT_NO_FATAL_FAILURE(TestPoolFreeing(ctx.pool, 5 * kChunkSize / 2, kChunkSize / 2));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_after}));

  // Double-frees should be no-ops.
  ASSERT_NO_FATAL_FAILURE(TestPoolFreeing(ctx.pool, kChunkSize, kChunkSize));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_after}));

  ASSERT_NO_FATAL_FAILURE(TestPoolFreeing(ctx.pool, kChunkSize, kChunkSize / 2));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_after}));

  ASSERT_NO_FATAL_FAILURE(TestPoolFreeing(ctx.pool, 3 * kChunkSize / 2, kChunkSize / 2));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_after}));

  ASSERT_NO_FATAL_FAILURE(TestPoolFreeing(ctx.pool, 2 * kChunkSize, kChunkSize));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected_after}));
}

TEST(MemallocPoolTests, FreedAllocations) {
  PoolContext ctx;
  Range ranges[] = {
      // free RAM: [0, 2*kChunkSize)
      {
          .addr = 0,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  const Range expected[] = {
      // bookkeeping: [0, kChunkSize)
      {
          .addr = 0,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      // free RAM: [kChunkSize, 2*kChunkSize)
      {
          .addr = kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  constexpr auto allocate_then_free = [](Pool& pool, uint64_t size) {
    auto result = pool.Allocate(Type::kPoolTestPayload, size, 1);
    ASSERT_FALSE(result.is_error());
    uint64_t addr = std::move(result).value();
    EXPECT_FALSE(pool.Free(addr, size).is_error());
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));

  ASSERT_NO_FATAL_FAILURE(allocate_then_free(ctx.pool, 1));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));

  ASSERT_NO_FATAL_FAILURE(allocate_then_free(ctx.pool, 2));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));

  ASSERT_NO_FATAL_FAILURE(allocate_then_free(ctx.pool, 4));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));

  ASSERT_NO_FATAL_FAILURE(allocate_then_free(ctx.pool, 8));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));

  ASSERT_NO_FATAL_FAILURE(allocate_then_free(ctx.pool, 16));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));

  ASSERT_NO_FATAL_FAILURE(allocate_then_free(ctx.pool, kChunkSize / 2));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));

  ASSERT_NO_FATAL_FAILURE(allocate_then_free(ctx.pool, kChunkSize));
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
}

TEST(MemallocPoolTests, RamSubrangeUpdates) {
  PoolContext ctx;
  Range ranges[] = {
      // RAM: [0, 3*kChunkSize)
      {
          .addr = 0,
          .size = 3 * kChunkSize,
          .type = Type::kFreeRam,
      },
      // data ZBI: [3*kChunkSize, 4*kChunkSize)
      {
          .addr = 3 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kDataZbi,
      },
      // RAM: [4*kChunkSize, 5*kChunkSize)
      {
          .addr = 4 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
      // RAM: [6*kChunkSize, 7*kChunkSize)
      {
          .addr = 6 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

  {
    const Range expected[] = {
        // bookkeeping: [0, kChunkSize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        // RAM: [kChunkSize, 3*kChunkSize)
        {
            .addr = kChunkSize,
            .size = 2 * kChunkSize,
            .type = Type::kFreeRam,
        },
        // data ZBI: [3*kChunkSize, 4*kChunkSize)
        {
            .addr = 3 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kDataZbi,
        },
        // RAM: [4*kChunkSize, 5*kChunkSize)
        {
            .addr = 4 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        // RAM: [6*kChunkSize, 7*kChunkSize)
        {
            .addr = 6 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
  }

  // Updating can happen across partial allocated and free RAM types.
  {
    ASSERT_NO_FATAL_FAILURE(TestPoolRamSubrangeUpdating(ctx.pool, Type::kPoolTestPayload,
                                                        3 * kChunkSize / 2, 2 * kChunkSize));

    const Range expected[] = {
        // bookkeeping: [0, kChunkSize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        // RAM: [kChunkSize, 3*kChunkSize/2)
        {
            .addr = kChunkSize,
            .size = kChunkSize / 2,
            .type = Type::kFreeRam,
        },
        // test payload: [(3/2)*kChunkSize, (7/2)*kChunkSize)
        {
            .addr = 3 * kChunkSize / 2,
            .size = 2 * kChunkSize,
            .type = Type::kPoolTestPayload,
        },
        // data ZBI: [(7/2)*kChunkSize, 4*kChunkSize)
        {
            .addr = 7 * kChunkSize / 2,
            .size = kChunkSize / 2,
            .type = Type::kDataZbi,
        },
        // RAM: [4*kChunkSize, 5*kChunkSize)
        {
            .addr = 4 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        // RAM: [6*kChunkSize, 7*kChunkSize)
        {
            .addr = 6 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
  }

  // Updating can happen across whole allocated and free RAM types.
  {
    ASSERT_NO_FATAL_FAILURE(
        TestPoolRamSubrangeUpdating(ctx.pool, Type::kPoolTestPayload, kChunkSize, 3 * kChunkSize));

    const Range expected[] = {
        // bookkeeping: [0, kChunkSize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        // test payload: [kChunkSize, 4*kChunkSize)
        // Updated.
        {
            .addr = kChunkSize,
            .size = 3 * kChunkSize,
            .type = Type::kPoolTestPayload,
        },
        // RAM: [4*kChunkSize, 5*kChunkSize)
        {
            .addr = 4 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        // RAM: [6*kChunkSize, 7*kChunkSize)
        {
            .addr = 6 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
  }

  // Updating can happen across holes.
  {
    ASSERT_NO_FATAL_FAILURE(TestPoolRamSubrangeUpdating(ctx.pool, Type::kPoolTestPayload,
                                                        4 * kChunkSize, 3 * kChunkSize));

    const Range expected[] = {
        // bookkeeping: [0, kChunkSize)
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        // test payload: [kChunkSize, 5*kChunkSize)
        // Updated.
        {
            .addr = kChunkSize,
            .size = 4 * kChunkSize,
            .type = Type::kPoolTestPayload,
        },
        // test payload: [6*kChunkSize, 7*kChunkSize)
        // Updated.
        {
            .addr = 6 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolTestPayload,
        },
    };
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
  }

  // Updating can even overwrite tracked bookkeeping.
  {
    ASSERT_NO_FATAL_FAILURE(TestPoolRamSubrangeUpdating(ctx.pool, Type::kPoolTestPayload,
                                                        kDefaultMinAddr, 7 * kChunkSize));

    const Range expected[] = {
        // test payload: [0, 5*kChunkSize)
        // Updated.
        {
            .addr = 0,
            .size = 5 * kChunkSize,
            .type = Type::kPoolTestPayload,
        },
        // test payload: [6*kChunkSize, 7*kChunkSize)
        // Updated.
        {
            .addr = 6 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolTestPayload,
        },
    };
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {expected}));
  }
}

TEST(MemallocPoolTests, Resizing) {
  constexpr uint64_t kMinAlignment = kChunkSize;

  // kNewSize == kOldSize
  {
    constexpr uint64_t kOldAddr = kChunkSize;
    constexpr uint64_t kOldSize = kChunkSize;

    constexpr Range kRange{
        .addr = kOldAddr,
        .size = kOldSize,
        .type = Type::kPoolTestPayload,
    };

    PoolContext ctx;
    Range ranges[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        kRange,
    };

    const Range post_resize[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        {
            .addr = kOldAddr,
            .size = kOldSize,
            .type = Type::kPoolTestPayload,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

    ASSERT_NO_FATAL_FAILURE(TestPoolResizing(ctx.pool, kRange, kOldSize, kMinAlignment, kOldAddr));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {post_resize}));

    // Resizing with a smaller alignment should be a no-opt.
    ASSERT_NO_FATAL_FAILURE(
        TestPoolResizing(ctx.pool, kRange, kOldSize, kMinAlignment / 2, kOldAddr));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {post_resize}));
  }

  // kNewSize < kOldSize
  {
    constexpr uint64_t kOldAddr = kChunkSize;
    constexpr uint64_t kOldSize = kChunkSize;
    constexpr uint64_t kNewSize = kChunkSize / 2;

    constexpr Range kRange{
        .addr = kOldAddr,
        .size = kOldSize,
        .type = Type::kPoolTestPayload,
    };

    PoolContext ctx;
    Range ranges[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        kRange,
    };

    const Range post_resize[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        {
            .addr = kOldAddr,
            .size = kNewSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = kOldAddr + kNewSize,
            .size = kChunkSize / 2,
            .type = Type::kFreeRam,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

    ASSERT_NO_FATAL_FAILURE(TestPoolResizing(ctx.pool, kRange, kNewSize, kMinAlignment, kOldAddr));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {post_resize}));
  }

  // kNewSize > kOldSize
  // Room for extension in-place.
  // No coalesced ranges.
  {
    constexpr uint64_t kOldAddr = kChunkSize;
    constexpr uint64_t kOldSize = kChunkSize;
    constexpr uint64_t kNewSize = 2 * kChunkSize;

    constexpr Range kRange{
        .addr = kOldAddr,
        .size = kOldSize,
        .type = Type::kPoolTestPayload,
    };

    PoolContext ctx;
    Range ranges[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        kRange,
        {
            .addr = 2 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    const Range post_resize[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        {
            .addr = kOldAddr,
            .size = kNewSize,
            .type = Type::kPoolTestPayload,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

    ASSERT_NO_FATAL_FAILURE(TestPoolResizing(ctx.pool, kRange, kNewSize, kMinAlignment, kOldAddr));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {post_resize}));
  }

  // kNewSize > kOldSize
  // Room for extension in-place.
  // Coalesced range on left.
  {
    constexpr uint64_t kOldAddr = 2 * kChunkSize;
    constexpr uint64_t kOldSize = kChunkSize;
    constexpr uint64_t kNewSize = 2 * kChunkSize;

    constexpr Range kRange{
        .addr = kOldAddr,
        .size = kOldSize,
        .type = Type::kPoolTestPayload,
    };

    PoolContext ctx;
    Range ranges[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        {
            .addr = kChunkSize,
            .size = kChunkSize + kOldSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = 3 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
    };

    const Range post_resize[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        {
            .addr = kChunkSize,
            .size = kChunkSize + kNewSize,
            .type = Type::kPoolTestPayload,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

    ASSERT_NO_FATAL_FAILURE(TestPoolResizing(ctx.pool, kRange, kNewSize, kMinAlignment, kOldAddr));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {post_resize}));
  }

  // kNewSize > kOldSize
  // No wiggle room; must reallocate into discontiguous memory.
  // No coalesced ranges.
  {
    constexpr uint64_t kOldAddr = kChunkSize;
    constexpr uint64_t kOldSize = kChunkSize;
    constexpr uint64_t kNewSize = 2 * kChunkSize;
    constexpr uint64_t kNewAddr = 10 * kChunkSize;

    constexpr Range kRange{
        .addr = kOldAddr,
        .size = kOldSize,
        .type = Type::kPoolTestPayload,
    };

    PoolContext ctx;
    Range ranges[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        kRange,
        {
            .addr = kNewAddr,
            .size = kNewSize,
            .type = Type::kFreeRam,
        },
    };

    const Range post_resize[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        {
            .addr = kOldAddr,
            .size = kOldSize,
            .type = Type::kFreeRam,
        },
        {
            .addr = kNewAddr,
            .size = kNewSize,
            .type = Type::kPoolTestPayload,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

    ASSERT_NO_FATAL_FAILURE(TestPoolResizing(ctx.pool, kRange, kNewSize, kMinAlignment, kNewAddr));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {post_resize}));
  }

  // kNewSize > kOldSize
  // No wiggle room; must reallocate into discontiguous memory.
  // Coalesced range on left.
  {
    constexpr uint64_t kOldAddr = 2 * kChunkSize;
    constexpr uint64_t kOldSize = kChunkSize;
    constexpr uint64_t kNewSize = 2 * kChunkSize;
    constexpr uint64_t kNewAddr = 10 * kChunkSize;

    constexpr Range kRange{
        .addr = kOldAddr,
        .size = kOldSize,
        .type = Type::kPoolTestPayload,
    };

    PoolContext ctx;
    Range ranges[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        {
            .addr = kChunkSize,
            .size = kChunkSize + kOldSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = kNewAddr,
            .size = kNewSize,
            .type = Type::kFreeRam,
        },
    };

    const Range post_resize[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        {
            .addr = kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = kOldAddr,
            .size = kOldSize,
            .type = Type::kFreeRam,
        },
        {
            .addr = kNewAddr,
            .size = kNewSize,
            .type = Type::kPoolTestPayload,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

    ASSERT_NO_FATAL_FAILURE(TestPoolResizing(ctx.pool, kRange, kNewSize, kMinAlignment, kNewAddr));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {post_resize}));
  }

  // kNewSize > kOldSize
  // No wiggle room; must reallocate into discontiguous memory.
  // Coalesced range on right.
  {
    constexpr uint64_t kOldAddr = kChunkSize;
    constexpr uint64_t kOldSize = kChunkSize;
    constexpr uint64_t kNewSize = 2 * kChunkSize;
    constexpr uint64_t kNewAddr = 10 * kChunkSize;

    constexpr Range kRange{
        .addr = kOldAddr,
        .size = kOldSize,
        .type = Type::kPoolTestPayload,
    };

    PoolContext ctx;
    Range ranges[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        {
            .addr = kOldAddr,
            .size = kOldSize + kChunkSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = kNewAddr,
            .size = kNewSize,
            .type = Type::kFreeRam,
        },
    };

    const Range post_resize[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        {
            .addr = kOldAddr,
            .size = kOldSize,
            .type = Type::kFreeRam,
        },
        {
            .addr = 2 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = kNewAddr,
            .size = kNewSize,
            .type = Type::kPoolTestPayload,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

    ASSERT_NO_FATAL_FAILURE(TestPoolResizing(ctx.pool, kRange, kNewSize, kMinAlignment, kNewAddr));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {post_resize}));
  }

  // kNewSize > kOldSize
  // No wiggle room; must reallocate into discontiguous memory.
  // Coalesced ranges on both sides.
  {
    constexpr uint64_t kOldAddr = 2 * kChunkSize;
    constexpr uint64_t kOldSize = kChunkSize;
    constexpr uint64_t kNewSize = 2 * kChunkSize;
    constexpr uint64_t kNewAddr = 10 * kChunkSize;

    constexpr Range kRange{
        .addr = kOldAddr,
        .size = kOldSize,
        .type = Type::kPoolTestPayload,
    };

    PoolContext ctx;
    Range ranges[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        {
            .addr = kChunkSize,
            .size = kChunkSize + kOldSize + kChunkSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = kNewAddr,
            .size = kNewSize,
            .type = Type::kFreeRam,
        },
    };

    const Range post_resize[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        {
            .addr = kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = kOldAddr,
            .size = kOldSize,
            .type = Type::kFreeRam,
        },
        {
            .addr = 3 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = kNewAddr,
            .size = kNewSize,
            .type = Type::kPoolTestPayload,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

    ASSERT_NO_FATAL_FAILURE(TestPoolResizing(ctx.pool, kRange, kNewSize, kMinAlignment, kNewAddr));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {post_resize}));
  }

  // kNewSize > kOldSize
  // Must reallocate into preceding range.
  // No coalesced range on right.
  {
    constexpr uint64_t kOldAddr = 2 * kChunkSize;
    constexpr uint64_t kOldSize = kChunkSize;
    constexpr uint64_t kNewSize = 3 * kChunkSize / 2;
    constexpr uint64_t kNewAddr = kChunkSize;

    constexpr Range kRange{
        .addr = kOldAddr,
        .size = kOldSize,
        .type = Type::kPoolTestPayload,
    };

    PoolContext ctx;
    Range ranges[] = {
        {
            .addr = 0,
            .size = 2 * kChunkSize,
            .type = Type::kFreeRam,
        },
        kRange,
    };

    const Range post_resize[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        {
            .addr = kNewAddr,
            .size = kNewSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = kNewAddr + kNewSize,
            .size = kChunkSize / 2,
            .type = Type::kFreeRam,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

    ASSERT_NO_FATAL_FAILURE(TestPoolResizing(ctx.pool, kRange, kNewSize, kMinAlignment, kNewAddr));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {post_resize}));
  }

  // kNewSize > kOldSize
  // Must reallocate into preceding range.
  // Coalesced range on right.
  {
    constexpr uint64_t kOldAddr = 2 * kChunkSize;
    constexpr uint64_t kOldSize = kChunkSize;
    constexpr uint64_t kNewSize = 3 * kChunkSize / 2;
    constexpr uint64_t kNewAddr = kChunkSize;

    constexpr Range kRange{
        .addr = kOldAddr,
        .size = kOldSize,
        .type = Type::kPoolTestPayload,
    };

    PoolContext ctx;
    Range ranges[] = {
        {
            .addr = 0,
            .size = 2 * kChunkSize,
            .type = Type::kFreeRam,
        },
        {
            .addr = kOldAddr,
            .size = kOldSize + kChunkSize,
            .type = Type::kPoolTestPayload,
        },
    };

    const Range post_resize[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        {
            .addr = kNewAddr,
            .size = kNewSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = kNewAddr + kNewSize,
            .size = kChunkSize / 2,
            .type = Type::kFreeRam,
        },
        {
            .addr = kOldAddr + kOldSize,
            .size = kChunkSize,
            .type = Type::kPoolTestPayload,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

    ASSERT_NO_FATAL_FAILURE(TestPoolResizing(ctx.pool, kRange, kNewSize, kMinAlignment, kNewAddr));
    ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {post_resize}));
  }

  // kNewSize > kOldSize
  // OOM.
  {
    constexpr uint64_t kOldAddr = kChunkSize;
    constexpr uint64_t kOldSize = kChunkSize;
    constexpr uint64_t kNewSize = 2 * kChunkSize;

    constexpr Range kRange{
        .addr = kOldAddr,
        .size = kOldSize,
        .type = Type::kPoolTestPayload,
    };

    PoolContext ctx;
    Range ranges[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kFreeRam,
        },
        kRange,
        {
            .addr = 2 * kChunkSize,
            .size = kChunkSize / 2,
            .type = Type::kFreeRam,
        },
    };

    ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

    ASSERT_NO_FATAL_FAILURE(
        TestPoolResizing(ctx.pool, kRange, kNewSize, kMinAlignment, std::nullopt));
  }
}

TEST(MemallocPoolTests, TotalRamTruncation) {
  PoolContext ctx;
  Range ranges[] = {
      // free RAM: [0, 2*kChunkSize)
      {
          .addr = 0,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

  //
  // Truncation to the current amount of RAM should be a no-op.
  //

  ASSERT_TRUE(ctx.pool.TruncateTotalRam(2 * kChunkSize).is_ok());

  constexpr Range kUnrestricted[] = {
      // bookkeeping: [0, kChunkSize)
      {
          .addr = 0,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      // free RAM: [kChunkSize, 2*kChunkSize)
      {
          .addr = kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {kUnrestricted}));

  //
  // Truncation by a half of a chunk should result in the second half of the
  // free RAM range being removed.
  //

  ASSERT_TRUE(ctx.pool.TruncateTotalRam(3 * kChunkSize / 2).is_ok());

  constexpr Range kChunkPartlyTruncated[] = {
      // bookkeeping: [0, kChunkSize)
      {
          .addr = 0,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      // free RAM: kChunkSize, 3*kChunkSize/2)
      {
          .addr = kChunkSize,
          .size = kChunkSize / 2,
          .type = Type::kFreeRam,
      },
      // truncated RAM: [3*kChunkSize/2, 2*kChunkSize)
      {
          .addr = 3 * kChunkSize / 2,
          .size = kChunkSize / 2,
          .type = Type::kTruncatedRam,
      },
  };
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {kChunkPartlyTruncated}));

  //
  // Truncation by another half chunk should just remove the remaining RAM
  // range, even if allocated.
  //

  ASSERT_NO_FATAL_FAILURE(
      TestPoolAllocation(ctx.pool, Type::kPoolTestPayload, kChunkSize / 2, kChunkSize / 2));

  ASSERT_TRUE(ctx.pool.TruncateTotalRam(kChunkSize).is_ok());

  constexpr Range kChunkTruncated[] = {// bookkeeping: [0, kChunkSize)
                                       {
                                           .addr = 0,
                                           .size = kChunkSize,
                                           .type = Type::kPoolBookkeeping,
                                       },
                                       // truncated RAM: [kChunkSize, 2*kChunkSize)
                                       {
                                           .addr = kChunkSize,
                                           .size = kChunkSize,
                                           .type = Type::kTruncatedRam,
                                       }};
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {kChunkTruncated}));

  //
  // Even parts of bookkeeping space may be removed.
  //

  ASSERT_TRUE(ctx.pool.TruncateTotalRam(kChunkSize / 2).is_ok());

  constexpr Range kBookkeepingPartlyShaved[] = {// bookkeeping: [0, kChunkSize/2)
                                                {
                                                    .addr = 0,
                                                    .size = kChunkSize / 2,
                                                    .type = Type::kPoolBookkeeping,
                                                },
                                                // truncated RAM: [kChunkSize/2, 2*kChunkSize)
                                                {
                                                    .addr = kChunkSize / 2,
                                                    .size = 3 * kChunkSize / 2,
                                                    .type = Type::kTruncatedRam,
                                                }};
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {kBookkeepingPartlyShaved}));

  //
  // Even all of bookkeeping space may be removed.
  //

  constexpr Range kAllTruncated[] = {// truncated RAM: [0, 2*kChunkSize)
                                     {
                                         .addr = 0,
                                         .size = 2 * kChunkSize,
                                         .type = Type::kTruncatedRam,
                                     }};

  ASSERT_TRUE(ctx.pool.TruncateTotalRam(0).is_ok());
  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, kAllTruncated));

  ASSERT_NO_FATAL_FAILURE(TestPoolAllocation(ctx.pool, Type::kPhysScratch, kChunkSize, kChunkSize,
                                             {}, {}, /*alloc_error=*/true));
}

TEST(MemallocPoolTests, TotalRamTruncationWithPeripheralAndNvramRanges) {
  PoolContext ctx;
  Range ranges[] = {
      // free RAM: [0, 2*kChunkSize)
      {
          .addr = 0,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
      // peripheral: [2*kChunkSize, 3*kChunkSize)
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPeripheral,
      },
      // test payload: [10*kChunkSize, 12*kChunkSize)
      {
          .addr = 10 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kPoolTestPayload,
      },
      // free RAM: [12*kChunkSize, 13*kChunkSize)
      {
          .addr = 12 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
      // NVRAM: [20*kChunkSize, 21*kChunkSize)
      {
          .addr = 20 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kNvram,
      },
      // free RAM: [30*kChunkSize, 31*kChunkSize)
      {
          .addr = 30 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

  //
  // Truncating to 3 chunks should result in the following set of ranges,
  // leaving the peripheral and NVRAM ranges in place and reallocating all RAM
  // midway through the test payload.
  //

  ASSERT_TRUE(ctx.pool.TruncateTotalRam(3 * kChunkSize).is_ok());

  constexpr Range kExpected[] = {
      // pool bookkeeping: [0, kChunkSize)
      {
          .addr = 0,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      // free RAM: [kChunkSize, 2*kChunkSize)
      {
          .addr = kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
      // peripheral: [2*kChunkSize, 3*kChunkSize)
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPeripheral,
      },
      // test payload: [10*kChunkSize, 11*kChunkSize)
      {
          .addr = 10 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolTestPayload,
      },
      // truncated RAM: [10*kChunkSize, 13*kChunkSize)
      {
          .addr = 11 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kTruncatedRam,
      },
      // NVRAM: [20*kChunkSize, 21*kChunkSize)
      {
          .addr = 20 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kNvram,
      },
      // truncated RAM: [30*kChunkSize, 31*kChunkSize)
      {
          .addr = 30 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kTruncatedRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {kExpected}));

  //
  // Even after removing all RAM, we should still track the peripheral and
  // NVRAM ranges.
  //
  ASSERT_TRUE(ctx.pool.TruncateTotalRam(0).is_ok());

  constexpr Range kJustPeripheralAndNvram[] = {
      // truncated RAM: [0, 2*kChunkSize)
      {
          .addr = 0,
          .size = 2 * kChunkSize,
          .type = Type::kTruncatedRam,
      },
      // peripheral: [2*kChunkSize, 3*kChunkSize)
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPeripheral,
      },
      // truncated RAM: [10*kChunkSize, 13*kChunkSize)
      {
          .addr = 10 * kChunkSize,
          .size = 3 * kChunkSize,
          .type = Type::kTruncatedRam,
      },
      // NVRAM: [20*kChunkSize, 21*kChunkSize)
      {
          .addr = 20 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kNvram,
      },
      // truncated RAM: [30*kChunkSize, 31*kChunkSize)
      {
          .addr = 30 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kTruncatedRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolContents(ctx.pool, {kJustPeripheralAndNvram}));
}

TEST(MemallocPoolTests, OutOfMemory) {
  PoolContext ctx;
  Range ranges[] = {
      // free RAM: [0, 2*kChunkSize)
      {
          .addr = 0,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  ASSERT_NO_FATAL_FAILURE(Oom(ctx.pool));

  // Capture the state now, before any further operations are made, for an
  // integrity check later.
  std::vector<Range> before(ctx.pool.begin(), ctx.pool.end());

  // Allocations should now fail.
  {
    auto result = ctx.pool.Allocate(Type::kPoolTestPayload, 1, 1);
    ASSERT_TRUE(result.is_error());
  }

  // Same for frees that subdivide ranges. In this case, we can free one byte
  // from any of the allocated ranges (which were two bytes each).
  {
    auto it = std::find_if(ctx.pool.begin(), ctx.pool.end(), [](const Range& range) {
      return range.type != Type::kPoolBookkeeping && range.type != Type::kFreeRam && range.size > 1;
    });
    ASSERT_NE(ctx.pool.end(), it);
    ASSERT_NO_FATAL_FAILURE(TestPoolFreeing(ctx.pool, it->addr, 1, /*free_error=*/true));
  }

  // Ditto for any weak allocations that result in subdivision.
  {
    auto it = std::find_if(ctx.pool.begin(), ctx.pool.end(), [](const Range& range) {
      return range.type == Type::kFreeRam && range.size > 1;
    });
    ASSERT_NE(ctx.pool.end(), it);
    ASSERT_NO_FATAL_FAILURE(TestPoolRamSubrangeUpdating(ctx.pool, Type::kPoolTestPayload, it->addr,
                                                        1,
                                                        /*alloc_error=*/true));
  }

  // Now, after the failed operations, verify that the contents remain
  // unmodified.
  std::vector<Range> after(ctx.pool.begin(), ctx.pool.end());
  ASSERT_NO_FATAL_FAILURE(memalloc::testing::CompareRanges(before, after));
}

TEST(MemallocPoolTests, MarkAsPeripheralBeforeFirstRange) {
  PoolContext ctx;
  Range ranges[] = {
      // free ram: [10, 12)
      {
          .addr = 10 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
      // free ram: [12, 14)
      {
          .addr = 12 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  // peripheral: [8, 10)
  constexpr Range kPeripheralRange = {
      .addr = 8 * kChunkSize,
      .size = 2 * kChunkSize,
      .type = memalloc::Type::kPeripheral,
  };

  ASSERT_TRUE(ctx.pool.MarkAsPeripheral(kPeripheralRange).is_ok());

  constexpr Range kExpected[] = {
      kPeripheralRange,
      {
          .addr = 10 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 11 * kChunkSize,
          .size = 3 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, MarkAsPeripheralAfterLastRange) {
  PoolContext ctx;
  Range ranges[] = {
      // free ram: [10, 12)
      {
          .addr = 10 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
      // free ram: [12, 14)
      {
          .addr = 12 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  // peripheral: [14, 16)
  constexpr Range kPeripheralRange = {
      .addr = 14 * kChunkSize,
      .size = 2 * kChunkSize,
      .type = memalloc::Type::kPeripheral,
  };

  ASSERT_TRUE(ctx.pool.MarkAsPeripheral(kPeripheralRange).is_ok());
  constexpr Range kExpected[] = {
      {
          .addr = 10 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 11 * kChunkSize,
          .size = 3 * kChunkSize,
          .type = Type::kFreeRam,
      },
      kPeripheralRange,
  };

  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, MarkAsPeripheralBetweenRanges) {
  PoolContext ctx;
  Range ranges[] = {
      // free ram: [4, 6)
      {
          .addr = 4 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
      // free ram: [8, 10)
      {
          .addr = 8 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  // peripheral: [6, 8)
  constexpr Range kPeripheralRange = {
      .addr = 6 * kChunkSize,
      .size = 2 * kChunkSize,
      .type = memalloc::Type::kPeripheral,
  };

  ASSERT_TRUE(ctx.pool.MarkAsPeripheral(kPeripheralRange).is_ok());

  constexpr Range kExpected[] = {
      {
          .addr = 4 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 5 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
      kPeripheralRange,
      {
          .addr = 8 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, MarkAsPeripheralBetweenRangesMerging) {
  PoolContext ctx;
  Range ranges[] = {
      // free ram: [2, 4)]
      {
          .addr = 2 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
      // peripheral: [4,6)
      {
          .addr = 4 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      // peripheral: [7, 10)
      {
          .addr = 7 * kChunkSize,
          .size = 3 * kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      // free ram: [10, 12)
      {
          .addr = 10 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  // peripheral: [5, 8)
  constexpr Range kPeripheralRange = {
      .addr = 5 * kChunkSize,
      .size = 2 * kChunkSize,
      .type = memalloc::Type::kPeripheral,
  };

  ASSERT_TRUE(ctx.pool.MarkAsPeripheral(kPeripheralRange).is_ok());

  constexpr Range kExpected[] = {
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 3 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
      {
          .addr = 4 * kChunkSize,
          .size = 6 * kChunkSize,
          .type = Type::kPeripheral,
      },
      {
          .addr = 10 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, MarkAsPeripheralBetweenRangesNoMerging) {
  PoolContext ctx;
  Range ranges[] = {
      // free ram: [2, 4)
      {
          .addr = 2 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
      // peripheral: [4, 5)
      {
          .addr = 4 * kChunkSize,
          .size = kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      // peripheral: [8, 9)
      {
          .addr = 8 * kChunkSize,
          .size = kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      // free ram: [10, 12)
      {
          .addr = 10 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

  // peripheral: [6,7)
  constexpr Range kPeripheralRange = {
      .addr = 6 * kChunkSize,
      .size = kChunkSize,
      .type = memalloc::Type::kPeripheral,
  };

  ASSERT_TRUE(ctx.pool.MarkAsPeripheral(kPeripheralRange).is_ok());

  constexpr Range kExpected[] = {
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 3 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
      {
          .addr = 4 * kChunkSize,
          .size = kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      kPeripheralRange,
      {
          .addr = 8 * kChunkSize,
          .size = kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      {
          .addr = 10 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, MarkAsPeripheralBeforeRangesWithMerging) {
  PoolContext ctx;
  Range ranges[] = {
      // peripheral: [2, 3)
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      // peripheral: [4, 5)
      {
          .addr = 4 * kChunkSize,
          .size = kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      // free ram: [5, 8)
      {
          .addr = 5 * kChunkSize,
          .size = 3 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

  // peripheral: [1, 5)
  constexpr Range kPeripheralRange = {
      .addr = kChunkSize,
      .size = 4 * kChunkSize,
      .type = memalloc::Type::kPeripheral,
  };

  ASSERT_TRUE(ctx.pool.MarkAsPeripheral(kPeripheralRange).is_ok());

  constexpr Range kExpected[] = {
      {
          .addr = kChunkSize,
          .size = 4 * kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      {
          .addr = 5 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 6 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, MarkAsPeripheralAdjacentBounds) {
  PoolContext ctx;
  Range ranges[] = {
      // free ram: [2, 4)
      {
          .addr = 2 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = memalloc::Type::kFreeRam,
      },
      // peripheral: [4, 5)
      {
          .addr = 4 * kChunkSize,
          .size = kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      // peripheral: [8, 9)
      {
          .addr = 8 * kChunkSize,
          .size = kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      // free ram: [9, 12)
      {
          .addr = 9 * kChunkSize,
          .size = 3 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  // peripheral: [5, 8)
  constexpr Range kPeripheralRange = {
      .addr = 5 * kChunkSize,
      .size = 3 * kChunkSize,
      .type = memalloc::Type::kPeripheral,
  };

  ASSERT_TRUE(ctx.pool.MarkAsPeripheral(kPeripheralRange).is_ok());

  constexpr Range kExpected[] = {
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 3 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
      {
          .addr = 4 * kChunkSize,
          .size = 5 * kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      {
          .addr = 9 * kChunkSize,
          .size = 3 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, MarkAsPeripheralAdjacentBoundsWithIntersectingRange) {
  PoolContext ctx;
  Range ranges[] = {
      // free ram: [2, 4)
      {
          .addr = 2 * kChunkSize,
          .size = 2 * kChunkSize,
          .type = memalloc::Type::kFreeRam,
      },
      // peripheral: [4, 5)
      {
          .addr = 4 * kChunkSize,
          .size = kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },

      // peripheral: [6, 7)
      {
          .addr = 6 * kChunkSize,
          .size = kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },

      // peripheral: [8, 9)
      {
          .addr = 8 * kChunkSize,
          .size = kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },

      // free ram: [9, 12)
      {
          .addr = 9 * kChunkSize,
          .size = 3 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  // peripheral: [5, 8)
  constexpr Range kPeripheralRange = {
      .addr = 5 * kChunkSize,
      .size = 3 * kChunkSize,
      .type = memalloc::Type::kPeripheral,
  };

  ASSERT_TRUE(ctx.pool.MarkAsPeripheral(kPeripheralRange).is_ok());

  constexpr Range kExpected[] = {
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 3 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kFreeRam,
      },
      {
          .addr = 4 * kChunkSize,
          .size = 5 * kChunkSize,
          .type = memalloc::Type::kPeripheral,
      },
      {
          .addr = 9 * kChunkSize,
          .size = 3 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };
  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, MarkAsPeripheralOverlapsWithNonPeripheral) {
  PoolContext ctx;

  Range ranges[] = {
      // free ram: [0, 5)]
      {
          .addr = 0,
          .size = 4 * kChunkSize,
          .type = Type::kFreeRam,
      },
      // reserved: [5, 9)
      {
          .addr = 5 * kChunkSize,
          .size = 4 * kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 0,
                      .size = kChunkSize,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_error());

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 5 * kChunkSize,
                      .size = kChunkSize,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_error());
}

TEST(MemallocPoolTests, CoalescePeripheralSingleRunAtStart) {
  PoolContext ctx;

  // Try alignment with realistic values.
  constexpr uint64_t kKb = 1ull << 10;
  constexpr uint64_t kMb = 1ull << 20;
  constexpr uint64_t kGb = 1ull << 30;

  Range ranges[] = {
      // free ram: [3, 5)]
      {
          .addr = 3 * kGb,
          .size = 2 * kGb,
          .type = Type::kFreeRam,
      },
      // reserved: [5, 9)
      {
          .addr = 5 * kGb,
          .size = 4 * kGb,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 256 * kMb,
                      .size = 256 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 756 * kMb,
                      .size = 1 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());
  auto coalesce_result = ctx.pool.CoalescePeripherals(std::to_array<size_t>({30, 21, 12}));
  ASSERT_TRUE(coalesce_result.is_ok());
  constexpr Range kExpected[] = {
      {
          .addr = 0,
          .size = kGb,
          .type = Type::kPeripheral,
      },
      {
          .addr = 3 * kGb,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 3 * kGb + kChunkSize,
          .size = 6 * kGb - kChunkSize,
          .type = Type::kFreeRam,
      },
  };

  ctx.pool.PrintMemoryRanges("test");
  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, CoalescePeripheralWorseAlignment) {
  PoolContext ctx;

  // Try alignment with realistic values.
  constexpr uint64_t kKb = 1ull << 10;
  constexpr uint64_t kMb = 1ull << 20;
  constexpr uint64_t kGb = 1ull << 30;

  Range ranges[] = {
      // free ram: [3, 5.5)]
      {
          .addr = 3 * kGb,
          .size = 2 * kGb + 512 * kMb,
          .type = Type::kFreeRam,
      },
      // free ram: [6.5, 9)
      {
          .addr = 6 * kGb + 512 * kMb,
          .size = 4 * kGb,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));
  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 5 * kGb + 517 * kMb,
                      .size = 256 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 6 * kGb + 511 * kMb,
                      .size = 1 * kMb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());
  auto coalesce_result = ctx.pool.CoalescePeripherals(std::to_array<size_t>({30, 21, 12}));
  ASSERT_TRUE(coalesce_result.is_ok());
  constexpr Range kExpected[] = {
      {
          .addr = 3 * kGb,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 3 * kGb + kChunkSize,
          .size = 2 * kGb + 512 * kMb - kChunkSize,
          .type = Type::kFreeRam,
      },
      {
          // Aligned to 2 MB boundary since 1GB is not feasible.
          .addr = 5 * kGb + 516 * kMb,
          .size = 1020 * kMb,
          .type = Type::kPeripheral,
      },
      {
          .addr = 6 * kGb + 512 * kMb,
          .size = 4 * kGb,
          .type = Type::kFreeRam,
      },
  };

  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, CoalescePeripheralSingleRunAtMiddle) {
  PoolContext ctx;

  // Try alignment with realistic values.
  constexpr uint64_t kKb = 1ull << 10;
  constexpr uint64_t kMb = 1ull << 20;
  constexpr uint64_t kGb = 1ull << 30;

  Range ranges[] = {
      // free ram: [3, 5)]
      {
          .addr = 3 * kGb,
          .size = 2 * kGb,
          .type = Type::kFreeRam,
      },
      // reserved: [5, 9)
      {
          .addr = 7 * kGb,
          .size = 2 * kGb,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 5 * kGb + 256 * kMb,
                      .size = 256 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 6 * kGb + 756 * kMb,
                      .size = 1 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());
  auto coalesce_result = ctx.pool.CoalescePeripherals(std::to_array<size_t>({30, 21, 12}));
  ASSERT_TRUE(coalesce_result.is_ok());
  constexpr Range kExpected[] = {
      {
          .addr = 3 * kGb,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 3 * kGb + kChunkSize,
          .size = 2 * kGb - kChunkSize,
          .type = Type::kFreeRam,
      },

      {
          .addr = 5 * kGb,
          .size = 2 * kGb,
          .type = Type::kPeripheral,
      },
      {
          .addr = 7 * kGb,
          .size = 2 * kGb,
          .type = Type::kFreeRam,
      },
  };

  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, CoalescePeripheralSingleRunAtEnd) {
  PoolContext ctx;

  // Try alignment with realistic values.
  constexpr uint64_t kKb = 1ull << 10;
  constexpr uint64_t kMb = 1ull << 20;
  constexpr uint64_t kGb = 1ull << 30;

  Range ranges[] = {
      // free ram: [3, 5)]
      {
          .addr = 3 * kGb,
          .size = 2 * kGb,
          .type = Type::kFreeRam,
      },
      // reserved: [5, 9)
      {
          .addr = 7 * kGb,
          .size = 2 * kGb,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 9 * kGb + 256 * kMb,
                      .size = 256 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 15 * kGb + 756 * kMb,
                      .size = 1 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());
  auto coalesce_result = ctx.pool.CoalescePeripherals(std::to_array<size_t>({30, 21, 12}));
  ASSERT_TRUE(coalesce_result.is_ok());
  constexpr Range kExpected[] = {
      {
          .addr = 3 * kGb,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 3 * kGb + kChunkSize,
          .size = 2 * kGb - kChunkSize,
          .type = Type::kFreeRam,
      },
      {
          .addr = 7 * kGb,
          .size = 2 * kGb,
          .type = Type::kFreeRam,
      },
      {
          .addr = 9 * kGb,
          .size = 7 * kGb,
          .type = Type::kPeripheral,
      },
  };

  TestPoolContents(ctx.pool, kExpected);
}

TEST(MemallocPoolTests, CoalescePeripheralMultipleRuns) {
  PoolContext ctx;

  // Try alignment with realistic values.
  constexpr uint64_t kKb = 1ull << 10;
  constexpr uint64_t kMb = 1ull << 20;
  constexpr uint64_t kGb = 1ull << 30;

  Range ranges[] = {
      // free ram: [3, 5)]
      {
          .addr = 3 * kGb,
          .size = 2 * kGb,
          .type = Type::kFreeRam,
      },
      // free ram: [5, 9)
      {
          .addr = 7 * kGb,
          .size = 2 * kGb,
          .type = Type::kFreeRam,
      },
  };

  ASSERT_NO_FATAL_FAILURE(TestPoolInit(ctx.pool, {ranges}));

  // At start.
  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 0 * kGb + 256 * kMb,
                      .size = 256 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 1 * kGb + 512 * kMb,
                      .size = 256 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 2 * kGb + 256 * kMb,
                      .size = 256 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());

  // At middle
  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 5 * kGb + 256 * kMb,
                      .size = 256 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());
  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 6 * kGb,
                      .size = 18 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 6 * kGb + 756 * kMb,
                      .size = 15 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());

  // At end
  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 11 * kGb + 900 * kMb,
                      .size = 256 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());

  ASSERT_TRUE(ctx.pool
                  .MarkAsPeripheral({
                      .addr = 12 * kGb + 2 * kMb,
                      .size = 1 * kKb,
                      .type = memalloc::Type::kPeripheral,
                  })
                  .is_ok());

  auto coalesce_result = ctx.pool.CoalescePeripherals(std::to_array<size_t>({30, 21, 12}));
  ASSERT_TRUE(coalesce_result.is_ok());
  constexpr Range kExpected[] = {
      {
          .addr = 0 * kGb,
          .size = 3 * kGb,
          .type = Type::kPeripheral,
      },
      {
          .addr = 3 * kGb,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      {
          .addr = 3 * kGb + kChunkSize,
          .size = 2 * kGb - kChunkSize,
          .type = Type::kFreeRam,
      },

      {
          .addr = 5 * kGb,
          .size = 2 * kGb,
          .type = Type::kPeripheral,
      },

      {
          .addr = 7 * kGb,
          .size = 2 * kGb,
          .type = Type::kFreeRam,
      },
      {
          .addr = 11 * kGb,
          .size = 2 * kGb,
          .type = Type::kPeripheral,
      },
  };

  TestPoolContents(ctx.pool, kExpected);
}

TEST(RangeTest, IntersectsWith) {
  Range a = {
      .addr = 0,
      .size = 2,
      .type = memalloc::Type::kFreeRam,
  };

  Range b = {
      .addr = 2,
      .size = 4,
      .type = memalloc::Type::kFreeRam,
  };

  Range c = {
      .addr = 1,
      .size = 3,
      .type = memalloc::Type::kFreeRam,
  };

  EXPECT_TRUE(a.IntersectsWith(a));
  EXPECT_FALSE(a.IntersectsWith(b));
  EXPECT_TRUE(a.IntersectsWith(c));

  EXPECT_FALSE(b.IntersectsWith(a));
  EXPECT_TRUE(b.IntersectsWith(b));
  EXPECT_TRUE(b.IntersectsWith(c));

  EXPECT_TRUE(c.IntersectsWith(a));
  EXPECT_TRUE(c.IntersectsWith(b));
  EXPECT_TRUE(c.IntersectsWith(c));
}

}  // namespace
