// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/memalloc/range.h>
#include <lib/memalloc/testing/range.h>

#include <span>
#include <vector>

#include <gtest/gtest.h>

namespace {

using Range = memalloc::Range;
using Type = memalloc::Type;

constexpr uint64_t kChunkSize = 0x1000;

TEST(MemallocRangeTests, NormalizeRanges) {
  constexpr Range kRanges[] = {
      // RAM: [0, kChunkSize)
      {
          .addr = 0,
          .size = kChunkSize,
          .type = Type::kPoolBookkeeping,
      },
      // data ZBI: [kChunkSize, 2*kChunkSize)
      {
          .addr = kChunkSize,
          .size = kChunkSize,
          .type = Type::kDataZbi,
      },
      // test payload: [2*kChunkSize, 3*kChunkSize)
      {
          .addr = 2 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolTestPayload,
      },
      // peripheral: [3*kChunkSize, 4*kChunkSize)
      {
          .addr = 3 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPeripheral,
      },
      // test payload: [5*kChunkSize, 6*kChunkSize)
      {
          .addr = 5 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPoolTestPayload,
      },
      // phys kernel: [6*kChunkSize, 7*kChunkSize)
      {
          .addr = 6 * kChunkSize,
          .size = kChunkSize,
          .type = Type::kPhysKernel,
      },
  };

  // Normalize away all ranges.
  {
    std::vector<Range> normalized;
    memalloc::NormalizeRanges(
        std::span{kRanges},
        [&normalized](const Range& range) {
          normalized.push_back(range);
          return true;
        },
        [](Type) { return std::nullopt; });
    EXPECT_TRUE(normalized.empty());
  }

  // Bail after the first range.
  {
    std::vector<Range> normalized;
    memalloc::NormalizeRanges(
        std::span{kRanges},
        [&normalized](const Range& range) {
          normalized.push_back(range);
          return false;
        },
        [](Type type) { return type; });
    EXPECT_EQ(1u, normalized.size());
  }

  // Normalize just RAM.
  {
    std::vector<Range> normalized;
    memalloc::NormalizeRam(std::span{kRanges}, [&normalized](const Range& range) {
      normalized.push_back(range);
      return true;
    });

    constexpr Range kExpected[] = {
        {
            .addr = 0,
            .size = 3 * kChunkSize,
            .type = Type::kFreeRam,
        },
        {
            .addr = 5 * kChunkSize,
            .size = 2 * kChunkSize,
            .type = Type::kFreeRam,
        },
    };
    memalloc::testing::CompareRanges(std::span{kExpected}, {normalized});
  }

  // Discard RAM.
  {
    std::vector<Range> normalized;
    memalloc::NormalizeRanges(
        std::span{kRanges},
        [&normalized](const Range& range) {
          normalized.push_back(range);
          return true;
        },
        [](Type type) {
          return (IsAllocatedType(type) || type == Type::kFreeRam) ? std::nullopt
                                                                   : std::make_optional(type);
        });

    constexpr Range kExpected[] = {
        {
            .addr = 3 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kPeripheral,
        },
    };
    memalloc::testing::CompareRanges(std::span{kExpected}, {normalized});
  }

  // Just keep pool test payloads and bookkeeping.
  {
    std::vector<Range> normalized;
    memalloc::NormalizeRanges(
        std::span{kRanges},
        [&normalized](const Range& range) {
          normalized.push_back(range);
          return true;
        },
        [](Type type) {
          return (type == Type::kPoolBookkeeping || type == Type::kPoolTestPayload)
                     ? std::make_optional(type)
                     : std::nullopt;
        });

    constexpr Range kExpected[] = {
        {
            .addr = 0,
            .size = kChunkSize,
            .type = Type::kPoolBookkeeping,
        },
        {
            .addr = 2 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolTestPayload,
        },
        {
            .addr = 5 * kChunkSize,
            .size = kChunkSize,
            .type = Type::kPoolTestPayload,
        },
    };
    memalloc::testing::CompareRanges(std::span{kExpected}, {normalized});
  }
}

}  // namespace
