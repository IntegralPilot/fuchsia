// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/memalloc/range.h>
#include <lib/memalloc/testing/range.h>

#include <cinttypes>
#include <span>

#include <gtest/gtest.h>
#include <vm/phys/arena.h>

// Whether to dump the selected arenas in ExpectArenas(). This is useful for the
// copy-paste updating of test expectations when max wastage or page bookking
// parameters are changed.
#define DUMP_SELECTED_ARENAS 0

namespace {

constexpr uint64_t kPageSize = 0x1000;

constexpr uint64_t kTestMaxWastedBytes = 0x1000;

//
// It's convenient in certain test cases to abstract away how much bookkeeping
// space is actually taken up within a given arena or to determine how many
// wasted pages will actually hit the wastage limit. This allows us to make test
// cases that deal in properties agnostic to the particular size of bookkeeping
// entries resilient to changes to that size. This is particularly convenient
// for the core test cases that deal more with arena and bookkeeping placement
// at a high level. On the other hand, this routine and constant are not
// intended for use in the test cases below that deal in simulating arena and
// bookkeeping selection on supported boards.
//
constexpr uint64_t BookkeepingSize(uint64_t arena_size) {
  uint64_t bookkeeping_entries = (arena_size / kPageSize) * kArenaPageBookkeepingSize;
  return (bookkeeping_entries + kPageSize - 1) & -kPageSize;
}

constexpr uint64_t kMaxNumWastedPagesUnderTestLimit =
    kTestMaxWastedBytes / kArenaPageBookkeepingSize;

// Compares expected arena and bookkeeping selection for a given set of input
// ranges with the actual.
void ExpectArenas(std::span<const memalloc::Range> ranges,
                  std::span<const PmmArenaSelection> expected,
                  std::span<const PmmArenaSelectionError> expected_errors = {},
                  uint64_t max_wasted_bytes = kTestMaxWastedBytes) {
  std::vector<PmmArenaSelection> actual;
  auto record_arena = [&actual](const PmmArenaSelection& arena) { actual.push_back(arena); };

  std::vector<PmmArenaSelectionError> actual_errors;
  auto record_error = [&actual_errors](const PmmArenaSelectionError& error) {
    actual_errors.push_back(error);
  };

  SelectPmmArenas<kPageSize>(ranges, record_arena, record_error, max_wasted_bytes);

  EXPECT_EQ(expected.size(), actual.size()) << "Unexpected number of arenas";
  for (size_t i = 0, size = std::min(expected.size(), actual.size()); i < size; ++i) {
    EXPECT_EQ(expected[i].arena.base, actual[i].arena.base) << "Arena #" << i;
    EXPECT_EQ(expected[i].arena.size, actual[i].arena.size) << "Arena #" << i;
    EXPECT_EQ(expected[i].bookkeeping.base, actual[i].bookkeeping.base) << "Arena #" << i;
    EXPECT_EQ(expected[i].bookkeeping.size, actual[i].bookkeeping.size) << "Arena #" << i;
    EXPECT_EQ(expected[i].wasted_bytes, actual[i].wasted_bytes) << "Arena #" << i;
  }

  EXPECT_EQ(expected_errors.size(), actual_errors.size()) << "Unexpected number of errors";
  for (size_t i = 0, size = std::min(expected_errors.size(), actual_errors.size()); i < size; ++i) {
    EXPECT_EQ(expected_errors[i].range.addr, actual_errors[i].range.addr) << "Error #" << i;
    EXPECT_EQ(expected_errors[i].range.size, actual_errors[i].range.size) << "Error #" << i;
    EXPECT_EQ(expected_errors[i].range.type, actual_errors[i].range.type) << "Error #" << i;
    EXPECT_EQ(expected_errors[i].type, actual_errors[i].type) << "Error #" << i;
  }

#if DUMP_SELECTED_ARENAS
  printf("--------------------------------------------------------------------------------\n");
  printf("constexpr PmmArenaSelection kExpected[] = {\n");
  for (const PmmArenaSelection& selected : actual) {
    printf("    // [%#" PRIx64 ", %#" PRIx64 "); bookkeeping @ [%#" PRIx64 ", %#" PRIx64 ")\n",
           selected.arena.base, selected.arena.end(), selected.bookkeeping.base,
           selected.bookkeeping.end());
    printf("    {\n");
    printf("        .arena = { .base = %#" PRIx64 ", .size = %#" PRIx64 " },\n",
           selected.arena.base, selected.arena.size);
    printf("        .bookkeeping = { .base = %#" PRIx64 ", .size = %#" PRIx64 " },\n",
           selected.bookkeeping.base, selected.bookkeeping.size);
    printf("        .wasted_bytes = %#" PRIx64 ",\n", selected.wasted_bytes);
    printf("    },\n");
  }
  printf("};\n");
  printf("--------------------------------------------------------------------------------\n");
#endif
}

// A thin wrapper around ExpectArenas() that provides the 'production' wastage
// limit. This is used in the practical test cases below that provide memory
// layouts that mirror specific supported boards.
void ExpectArenasInPractice(std::span<const memalloc::Range> ranges,
                            std::span<const PmmArenaSelection> expected,
                            std::span<const PmmArenaSelectionError> expected_errors = {}) {
  ExpectArenas(ranges, expected, expected_errors, kMaxWastedArenaBytes);
}

void ExpectAlignedAllocationsOrHoles(std::span<const memalloc::Range> input_ranges,
                                     std::span<const memalloc::Range> expected) {
  std::vector<memalloc::Range> actual;
  ForEachAlignedAllocationOrHole<kPageSize>(input_ranges, [&actual](const memalloc::Range& range) {
    actual.push_back(range);
    return true;
  });
  memalloc::testing::CompareRanges(expected, actual);
}

//
// Main test cases.
//
// Impractically small sizes are used below for readability. The semantics of
// the code under test is indifferent.
//

TEST(PmmArenaSelectionTests, NoRam) {
  // No ranges at all.
  ExpectArenas({}, {});

  // Only peripheral and undefined ranges.
  constexpr memalloc::Range kRanges[] = {
      // unknown: [0x0000, 0x5000)
      {
          .addr = 0x0000,
          .size = 0x5000,
          .type = static_cast<memalloc::Type>(memalloc::kMinAllocatedTypeValue - 1),
      },
      // peripheral: [0x8000, 0xa000)
      {
          .addr = 0x8000,
          .size = 0x2000,
          .type = memalloc::Type::kPeripheral,
      },
  };
  ExpectArenas({kRanges}, {});
}

TEST(PmmArenaSelectionTests, BookkeepingAvoidsAllocations) {
  constexpr memalloc::Range kRanges[] = {
      // kernel: [0x1000, 0x3000)
      {
          .addr = 0x1000,
          .size = 0x2000,
          .type = memalloc::Type::kKernel,
      },
      // data ZBI: [0x3000, 0x4000)
      {
          .addr = 0x3000,
          .size = 0x1000,
          .type = memalloc::Type::kDataZbi,
      },
      // free RAM: [0x4000, 0xa000)
      {
          .addr = 0x4000,
          .size = 0x6000,
          .type = memalloc::Type::kFreeRam,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      {
          .arena = {.base = 0x1000, .size = 0x9000},
          .bookkeeping = {.base = 0x9000, .size = 0x1000},
          .wasted_bytes = 0,
      },
  };

  ExpectArenas({kRanges}, {kExpected});
}

TEST(PmmArenaSelectionTests, BookkeepingInLargestFreeRamSubrange) {
  constexpr memalloc::Range kRanges[] = {
      // free RAM: [0x1000, 0x3000)
      {
          .addr = 0x1000,
          .size = 0x2000,
          .type = memalloc::Type::kFreeRam,
      },
      // kernel: [0x3000, 0x5000)
      {
          .addr = 0x3000,
          .size = 0x2000,
          .type = memalloc::Type::kKernel,
      },
      // free RAM: [0x5000, 0xa000)
      {
          .addr = 0x5000,
          .size = 0x5000,
          .type = memalloc::Type::kFreeRam,
      },
      // free RAM: [0xb000, 0xd000)
      {
          .addr = 0xb000,
          .size = 0x2000,
          .type = memalloc::Type::kFreeRam,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      {
          .arena = {.base = 0x1000, .size = 0xc000},
          .bookkeeping = {.base = 0x9000, .size = 0x1000},
          .wasted_bytes = kArenaPageBookkeepingSize,
      },
  };

  ExpectArenas({kRanges}, {kExpected});
}

TEST(PmmArenaSelectionTests, ArenasAndBookkeepingArePageAligned) {
  constexpr memalloc::Range kRanges[] = {

      // NVRAM: [0x0800, 0x1400)
      {
          .addr = 0x0800,
          .size = 0x0c00,
          .type = memalloc::Type::kNvram,
      },
      // free RAM: [0x1400, 0x4400)
      {
          .addr = 0x1400,
          .size = 0x3000,
          .type = memalloc::Type::kFreeRam,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      {
          .arena = {.base = 0x1000, .size = 0x3000},
          .bookkeeping = {.base = 0x3000, .size = 0x1000},
          .wasted_bytes = 0,
      },
  };

  ExpectArenas({kRanges}, {kExpected});
}

TEST(PmmArenaSelectionTests, ArenasAroundWastageLimit) {
  // We contrive addresses so that there is a gap just within the wastage limit
  // between the first and second ranges, but a gap that would result in
  // excessive waste between the second and third.
  constexpr uint64_t kFirstRangeStart = 0x0;
  constexpr uint64_t kFirstRangeEnd = 0x1000;

  constexpr uint64_t kSecondRangeStart =
      kFirstRangeEnd + (kMaxNumWastedPagesUnderTestLimit * kPageSize);
  constexpr uint64_t kSecondRangeEnd =
      kSecondRangeStart + ((kMaxNumWastedPagesUnderTestLimit + 2) * kPageSize);

  constexpr uint64_t kThirdRangeStart =
      kSecondRangeEnd + ((kMaxNumWastedPagesUnderTestLimit + 1) * kPageSize);
  constexpr uint64_t kThirdRangeEnd = kThirdRangeStart + 0x2000;

  constexpr memalloc::Range kRanges[] = {
      {
          .addr = kFirstRangeStart,
          .size = kFirstRangeEnd - kFirstRangeStart,
          .type = memalloc::Type::kFreeRam,
      },
      {
          .addr = kSecondRangeStart,
          .size = kSecondRangeEnd - kSecondRangeStart,
          .type = memalloc::Type::kFreeRam,
      },
      {
          .addr = kThirdRangeStart,
          .size = kThirdRangeEnd - kThirdRangeStart,
          .type = memalloc::Type::kFreeRam,
      },
  };

  // By design, we expect the first gap to be included in the first arena, but
  // not the second gap (resulting in a second arena).
  constexpr uint64_t kExpectedFirstArenaBookkeepingSize =
      BookkeepingSize(kSecondRangeEnd - kFirstRangeStart);
  constexpr uint64_t kExpectedSecondArenaBookkeepingSize =
      BookkeepingSize(kThirdRangeEnd - kThirdRangeStart);
  constexpr PmmArenaSelection kExpected[] = {
      {
          .arena = {.base = kFirstRangeStart, .size = kSecondRangeEnd - kFirstRangeStart},
          .bookkeeping = {.base = kSecondRangeEnd - kExpectedFirstArenaBookkeepingSize,
                          .size = kExpectedFirstArenaBookkeepingSize},
          .wasted_bytes = kMaxNumWastedPagesUnderTestLimit * kArenaPageBookkeepingSize,
      },
      {
          .arena = {.base = kThirdRangeStart, .size = kThirdRangeEnd - kThirdRangeStart},
          .bookkeeping = {.base = kThirdRangeEnd - kExpectedSecondArenaBookkeepingSize,
                          .size = kExpectedSecondArenaBookkeepingSize},
          .wasted_bytes = 0,
      },
  };

  ExpectArenas({kRanges}, {kExpected});
}

TEST(PmmArenaSelectionTests, ErrorNoBookkeepingSpace) {
  // The minimum wasteful hole size whose pages are divisible by 4.
  constexpr uint64_t kWastefulHoleSize = kPageSize * ((kMaxNumWastedPagesUnderTestLimit + 4) & -4);

  // We contrive three ranges such that the gap between the first and the second
  // is too large to bridge within an arena - resulting in us not being able to
  // find bookkeeping for the first - as well as the gap between the second and
  // the third indeed being bridgeable.
  constexpr uint64_t kFirstRangeStart = 0x1000;
  constexpr uint64_t kFirstRangeEnd = 0x2000;

  constexpr uint64_t kSecondRangeStart = kFirstRangeEnd + kWastefulHoleSize;
  constexpr uint64_t kSecondRangeEnd = kSecondRangeStart + (kWastefulHoleSize / 4);

  constexpr uint64_t kThirdRangeStart = kSecondRangeEnd + (kWastefulHoleSize / 4);
  constexpr uint64_t kThirdRangeEnd = kThirdRangeStart + (kWastefulHoleSize / 4);

  constexpr memalloc::Range kRanges[] = {
      {
          .addr = kFirstRangeStart,
          .size = kFirstRangeEnd - kFirstRangeStart,
          .type = memalloc::Type::kDataZbi,
      },
      {
          .addr = kSecondRangeStart,
          .size = kSecondRangeEnd - kSecondRangeStart,
          .type = memalloc::Type::kKernel,
      },
      {
          .addr = kThirdRangeStart,
          .size = kThirdRangeEnd - kThirdRangeStart,
          .type = memalloc::Type::kFreeRam,
      },
  };

  // The second and third range should comprise the only arena.
  constexpr uint64_t kExpectedBookkeepingSize = BookkeepingSize(kThirdRangeEnd - kSecondRangeStart);
  constexpr PmmArenaSelection kExpected[] = {
      {
          .arena = {.base = kSecondRangeStart, .size = kThirdRangeEnd - kSecondRangeStart},
          .bookkeeping = {.base = kThirdRangeEnd - kExpectedBookkeepingSize,
                          .size = kExpectedBookkeepingSize},
          .wasted_bytes = ((kWastefulHoleSize / 4) / kPageSize) * kArenaPageBookkeepingSize,
      },
  };

  constexpr PmmArenaSelectionError kExpectedErrors[] = {
      {
          .range = kRanges[0],
          .type = PmmArenaSelectionError::Type::kNoBookkeepingSpace,
      },
  };

  ExpectArenas({kRanges}, {kExpected}, {kExpectedErrors});
}

TEST(PmmArenaSelectionTests, ErrorTooSmall) {
  // Size < 0x1000
  {
    constexpr memalloc::Range kRanges[] = {
        // free RAM: [0x1000, 0x1800)
        {
            .addr = 0x1000,
            .size = 0x0800,
            .type = memalloc::Type::kFreeRam,
        },
    };

    constexpr PmmArenaSelectionError kExpectedErrors[] = {
        {
            .range = kRanges[0],
            .type = PmmArenaSelectionError::Type::kTooSmall,
        },
    };
    ExpectArenas({kRanges}, {}, {kExpectedErrors});
  }

  // Size < 0x2000
  {
    constexpr memalloc::Range kRanges[] = {
        // free RAM: [0x1000, 0x2800)
        {
            .addr = 0x1000,
            .size = 0x1800,
            .type = memalloc::Type::kFreeRam,
        },
    };

    constexpr PmmArenaSelectionError kExpectedErrors[] = {
        {
            .range = kRanges[0],
            .type = PmmArenaSelectionError::Type::kTooSmall,
        },
    };
    ExpectArenas({kRanges}, {}, {kExpectedErrors});
  }

  // Size < 0x2000 after aligning
  {
    constexpr memalloc::Range kRanges[] = {
        // free RAM: [0x0800, 0x2800)
        {
            .addr = 0x0800,
            .size = 0x2000,
            .type = memalloc::Type::kFreeRam,
        },
    };

    constexpr PmmArenaSelectionError kExpectedErrors[] = {
        {
            .range = kRanges[0],
            .type = PmmArenaSelectionError::Type::kTooSmall,
        },
    };
    ExpectArenas({kRanges}, {}, {kExpectedErrors});
  }
}

//
// Additional test cases that mirror the memory configurations of supported
// boards. These should use `ExpectArenasInPractice()` to best simulate
// practical outcomes of booting on these boards.
//
// For simplicity, these cases do not feature allocated types (beyon NVRAM)
// (which should only affect bookkeeping placement).
//

TEST(PmmArenaSelectionTests, QemuTcgArm) {
  constexpr memalloc::Range kRanges[] = {
      // peripheral: [0x0000'0000, 0x4000'0000)
      {
          .addr = 0x0000'0000,
          .size = 0x4000'0000,
          .type = memalloc::Type::kPeripheral,
      },
      // free RAM: [0x0'4000'0000, 0x1'3fff'0000)
      {
          .addr = 0x0'4000'0000,
          .size = 0x1'3fff'0000 - 0x0'4000'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // NVRAM: [0x1'3fff'0000, 0x1'4000'0000)
      {
          .addr = 0x1'3fff'0000,
          .size = 0x1'4000'0000 - 0x1'3fff'0000,
          .type = memalloc::Type::kNvram,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      // [0x4000'0000, 0x14000'0000); bookkeeping @ [0x1'3cff'0000, 0x1'3fff'0000)
      {
          .arena = {.base = 0x4000'0000, .size = 0x1'0000'0000},
          .bookkeeping = {.base = 0x1'3cff'0000, .size = 0x300'0000},
          .wasted_bytes = 0,
      },
  };

  ExpectArenasInPractice({kRanges}, {kExpected});
}

TEST(PmmArenaSelectionTests, QemuTcgX86) {
  constexpr memalloc::Range kRanges[] = {
      // free RAM: [0x0'0010'0000, 0x0'7ffd'f000)
      {
          .addr = 0x0'0010'0000,
          .size = 0x0'7ffd'f000 - 0x0'0010'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // free RAM: [0x1'0000'0000, 0x2'8000'0000)
      {
          .addr = 0x1'0000'0000,
          .size = 0x2'8000'0000 - 0x1'0000'0000,
          .type = memalloc::Type::kFreeRam,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      // [0x10'0000, 0x7ffd'f000); bookkeeping @ [0x7e7e'2000, 0x18f'd000)
      {
          .arena = {.base = 0x10'0000, .size = 0x7fed'f000},
          .bookkeeping = {.base = 0x7e7e'2000, .size = 0x17f'd000},
          .wasted_bytes = 0,
      },
      // [0x1'0000'0000, 0x2'8000'0000); bookkeeping @ [0x2'7e80'0000, 0x2'8000'0000)
      {
          .arena = {.base = 0x1'0000'0000, .size = 0x1'8000'0000},
          .bookkeeping = {.base = 0x2'7b80'0000, .size = 0x480'0000},
          .wasted_bytes = 0,
      },
  };

  ExpectArenasInPractice({kRanges}, {kExpected});
}

TEST(PmmArenaSelectionTests, QemuTcgRiscv) {
  constexpr memalloc::Range kRanges[] = {
      // free RAM: [0x8004'0000, 0x2'8000'0000)
      {
          .addr = 0x8004'0000,
          .size = 0x2'8000'0000 - 0x8004'0000,
          .type = memalloc::Type::kFreeRam,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      // [0x8004'0000, 0x2'8000'0000); bookkeeping @ [0x2'7a00'0000, 0x2'8000'0000)
      {
          .arena = {.base = 0x8004'0000, .size = 0x1'fffc'0000},
          .bookkeeping = {.base = 0x2'7a00'0000, .size = 0x600'0000},
          .wasted_bytes = 0,
      },
  };

  ExpectArenasInPractice({kRanges}, {kExpected});
}

TEST(PmmArenaSelectionTests, Nuc11) {
  constexpr memalloc::Range kRanges[] = {
      // free RAM: [0x0010'0000, 0x3780'4000)
      {
          .addr = 0x0010'0000,
          .size = 0x3780'4000 - 0x0010'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // free RAM: [0x417f'f000, 0x4180'0000)
      {
          .addr = 0x417f'f000,
          .size = 0x1000,
          .type = memalloc::Type::kFreeRam,
      },
      // free RAM: [0x1'0000'0000, 0x4'b080'0000)
      {
          .addr = 0x1'0000'0000,
          .size = 0x4'b080'0000 - 0x1'0000'0000,
          .type = memalloc::Type::kFreeRam,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      // [0x10'0000, 0x37804000); bookkeeping @ [0x36d9'e000, 0x3780'4000)
      {
          .arena = {.base = 0x10'0000, .size = 0x3770'4000},
          .bookkeeping = {.base = 0x36d9'e000, .size = 0xa6'6000},
          .wasted_bytes = 0,
      },
      // [0x1'0000'0000, 0x4'b080'0000); bookkeeping @ [0x4'a56e'8000, 0x4'b080'0000)
      {
          .arena = {.base = 0x1'0000'0000, .size = 0x3'b080'0000},
          .bookkeeping = {.base = 0x4'a56e'8000, .size = 0xb11'8000},
          .wasted_bytes = 0,
      },
  };

  constexpr PmmArenaSelectionError kExpectedErrors[] = {{
      .range = kRanges[1],
      .type = PmmArenaSelectionError::Type::kTooSmall,
  }};

  ExpectArenasInPractice({kRanges}, {kExpected}, {kExpectedErrors});
}

TEST(PmmArenaSelectionTests, Vim3) {
  constexpr memalloc::Range kRanges[] = {
      // free RAM: [0x0000'0000, 0x0500'0000)
      {
          .addr = 0x0000'0000,
          .size = 0x0500'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // free RAM: [0x0730'0000, 0x0740'0000)
      {
          .addr = 0x0730'0000,
          .size = 0x0740'0000 - 0x0730'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // free RAM: [0x0750'0000, 0xe000'0000)
      {
          .addr = 0x0750'0000,
          .size = 0xe000'0000 - 0x0750'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // peripheral: [0xfe00'0000,  0x1'0000'0000)
      {
          .addr = 0xfe00'0000,
          .size = 0x1'0000'0000 - 0xfe00'0000,
          .type = memalloc::Type::kPeripheral,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      // [0, 0xe000'0000); bookkeeping @ [0xdd60'0000, 0xe000'0000)
      {
          .arena = {.base = 0, .size = 0xe000'0000},
          .bookkeeping = {.base = 0xdd60'0000, .size = 0x2a0'0000},
          .wasted_bytes = 0x6c000,
      },
  };

  ExpectArenasInPractice({kRanges}, {kExpected});
}

TEST(PmmArenaSelectionTests, Astro) {
  constexpr memalloc::Range kRanges[] = {
      // free RAM: [0x0000'0000, 0x0500'0000)
      {
          .addr = 0x0000'0000,
          .size = 0x0500'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // free RAM: [0x0740'0000, 0x5f7f'e000)
      {
          .addr = 0x0740'0000,
          .size = 0x5f7f'e000 - 0x0740'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // NVRAM: [0x5f7f'e000, 0x5f80'0000)
      {
          .addr = 0x5f7f'e000,
          .size = 0x5f80'0000 - 0x5f7f'e000,
          .type = memalloc::Type::kNvram,
      },
      // peripheral: [0xf580'0000, 0x1'0000'0000)
      {
          .addr = 0xf580'0000,
          .size = 0x1'0000'0000 - 0xf580'0000,
          .type = memalloc::Type::kPeripheral,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      // [0, 0x5f80'0000); bookkeeping @ [0x5e61'6000, 0x5f7f'e000)
      {
          .arena = {.base = 0, .size = 0x5f80'0000},
          .bookkeeping = {.base = 0x5e61'6000, .size = 0x11e'8000},
          .wasted_bytes = 0x6'c000,
      },
  };

  ExpectArenasInPractice({kRanges}, {kExpected});
}

TEST(PmmArenaSelectionTests, Sherlock) {
  constexpr memalloc::Range kRanges[] = {
      // free RAM: [0x0010'0000, 0x0500'0000)
      {
          .addr = 0x0010'0000,
          .size = 0x0500'0000 - 0x0010'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // free RAM: [0x0740'0000, 0x5f7f'e000)
      {
          .addr = 0x0740'0000,
          .size = 0x5f7f'e000 - 0x0740'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // NVRAM: [0x5f7f'e000, 0x5f80'0000)
      {
          .addr = 0x5f7f'e000,
          .size = 0x5f80'0000 - 0x5f7f'e000,
          .type = memalloc::Type::kNvram,
      },
      // free RAM: [0x5f80'0000, 0x7680'0000)
      {
          .addr = 0x5f80'0000,
          .size = 0x7680'0000 - 0x5f80'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // free RAM: [0x7700'0000, 0x8000'0000)
      {
          .addr = 0x7700'0000,
          .size = 0x8000'0000 - 0x7700'0000,
          .type = memalloc::Type::kFreeRam,
      },

      // peripheral: [0xf580'0000, 0x1'0000'0000)
      {
          .addr = 0xf580'0000,
          .size = 0x1'0000'0000 - 0xf580'0000,
          .type = memalloc::Type::kPeripheral,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      // [0x10'0000, 0x8000'0000); bookkeeping @ [0x5e00'1000, 0x5f7f'e000)
      {
          .arena = {.base = 0x10'0000, .size = 0x7ff0'0000},
          .bookkeeping = {.base = 0x5e00'1000, .size = 0x17f'd000},
          .wasted_bytes = 0x8'4000,
      },
  };

  ExpectArenasInPractice({kRanges}, {kExpected});
}

TEST(PmmArenaSelectionTests, Nelson) {
  constexpr memalloc::Range kRanges[] = {
      // free RAM: [0x0000'0000, 0x0500'0000)
      {
          .addr = 0x0000'0000,
          .size = 0x0500'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // free RAM: [0x0740'0000, 0x5f7f'e000)
      {
          .addr = 0x0740'0000,
          .size = 0x5f7f'e000 - 0x0740'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // NVRAM: [0x5f7f'e000, 0x5f80'0000)
      {
          .addr = 0x5f7f'e000,
          .size = 0x5f80'0000 - 0x5f7f'e000,
          .type = memalloc::Type::kNvram,
      },
      // peripheral: [0x5f80'0000, 0x8000'0000)
      {
          .addr = 0x5f80'0000,
          .size = 0x8000'0000 - 0x5f80'0000,
          .type = memalloc::Type::kFreeRam,
      },
      // peripheral: [0xf580'0000, 0x1'0000'0000)
      {
          .addr = 0xf580'0000,
          .size = 0x1'0000'0000 - 0xf580'0000,
          .type = memalloc::Type::kPeripheral,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      // [0, 0x8000'0000); bookkeeping @ [0x5dff'e000, 0x5f7f'e000)
      {
          .arena = {.base = 0, .size = 0x8000'0000},
          .bookkeeping = {.base = 0x5dff'e000, .size = 0x180'0000},
          .wasted_bytes = 0x6'c000,
      },
  };

  ExpectArenasInPractice({kRanges}, {kExpected});
}

TEST(PmmArenaSelectionTests, SplitArenaBoundaryIssue) {
  // A regression test for the issue where an unaligned allocated range (like the
  // ZBI) could end just past the end of a split arena, causing PMM initialization
  // to fail. This demonstrates the behavior of SelectPmmArenas that caused the
  // mismatch: it rounds down the arena size (to 0x54000), leaving the end of the
  // unaligned ZBI (0x54f00) dangling outside the arena.
  constexpr memalloc::Range kRanges[] = {
      // free RAM: [0, 0x1000) (1 page)
      {
          .addr = 0x0,
          .size = 0x1000,
          .type = memalloc::Type::kFreeRam,
      },
      // kernel: [0x1000, 0x53000)
      {
          .addr = 0x1000,
          .size = 0x52000,
          .type = memalloc::Type::kKernel,
      },
      // data ZBI (unaligned): [0x53000, 0x54f00)
      {
          .addr = 0x53000,
          .size = 0x1f00,
          .type = memalloc::Type::kDataZbi,
      },
      // A large hole to force an arena split!
      // free RAM: [0x200000, 0x201000)
      {
          .addr = 0x200000,
          .size = 0x1000,
          .type = memalloc::Type::kFreeRam,
      },
  };

  constexpr PmmArenaSelection kExpected[] = {
      {
          .arena = {.base = 0x0, .size = 0x54000},
          .bookkeeping = {.base = 0x0, .size = 0x1000},
          .wasted_bytes = 0,
      },
  };

  constexpr PmmArenaSelectionError kExpectedErrors[] = {
      {
          .range =
              {
                  .addr = 0x200000,
                  .size = 0x1000,
                  .type = memalloc::Type::kFreeRam,
              },
          .type = PmmArenaSelectionError::Type::kTooSmall,
      },
  };

  ExpectArenas({kRanges}, {kExpected}, {kExpectedErrors});
}

TEST(ForEachAlignedAllocationOrHoleTests, Empty) { ExpectAlignedAllocationsOrHoles({}, {}); }

TEST(ForEachAlignedAllocationOrHoleTests, AlreadyAlignedRegions) {
  constexpr memalloc::Range kInputRanges[] = {
      {
          .addr = 0x0000,
          .size = 0x1000,
          .type = memalloc::Type::kDataZbi,
      },
      {
          .addr = 0x2000,
          .size = 0x1000,
          .type = memalloc::Type::kFreeRam,
      },
      {
          .addr = 0x3000,
          .size = 0x1000,
          .type = memalloc::Type::kKernel,
      },
  };
  constexpr memalloc::Range kExpected[] = {
      {
          .addr = 0x0000,
          .size = 0x1000,
          .type = memalloc::Type::kDataZbi,
      },
      {
          .addr = 0x1000,
          .size = 0x1000,
          .type = memalloc::Type::kReserved,
      },
      {
          .addr = 0x3000,
          .size = 0x1000,
          .type = memalloc::Type::kKernel,
      },
  };
  ExpectAlignedAllocationsOrHoles({kInputRanges}, {kExpected});
}

TEST(ForEachAlignedAllocationOrHoleTests, HoleAndFreeRamInSamePage) {
  // (RAM, hole) in page 1; RAM in page 2; (hole, RAM) in page 3
  constexpr memalloc::Range kInputRanges[] = {
      // free RAM: [0, 0x800)
      {
          .addr = 0x0000,
          .size = 0x0800,
          .type = memalloc::Type::kFreeRam,
      },
      // hole: [0x800, 0x1000)
      // free RAM: [0x1000, 0x2000)
      {
          .addr = 0x1000,
          .size = 0x1000,
          .type = memalloc::Type::kFreeRam,
      },
      // hole: [0x2000, 0x2800)
      // free RAM: [0x2800, 0x3000)
      {
          .addr = 0x2800,
          .size = 0x0800,
          .type = memalloc::Type::kFreeRam,
      },
  };
  constexpr memalloc::Range kExpected[] = {
      {
          .addr = 0x0000,
          .size = 0x1000,
          .type = memalloc::Type::kReserved,
      },
      {
          .addr = 0x2000,
          .size = 0x1000,
          .type = memalloc::Type::kReserved,
      },
  };
  ExpectAlignedAllocationsOrHoles({kInputRanges}, {kExpected});
}

TEST(ForEachAlignedAllocationOrHoleTests, AllocationAndFreeRamInSamePage) {
  // (RAM, allocation) in page 1; RAM in page 2; (allocation, RAM) in page 3
  constexpr memalloc::Range kInputRanges[] = {
      // free RAM: [0, 0x800)
      {
          .addr = 0x0000,
          .size = 0x0800,
          .type = memalloc::Type::kFreeRam,
      },
      // phys log: [0x800, 0x1000)
      {
          .addr = 0x0800,
          .size = 0x0800,
          .type = memalloc::Type::kPhysLog,
      },
      // free RAM: [0x1000, 0x2000)
      {
          .addr = 0x1000,
          .size = 0x1000,
          .type = memalloc::Type::kFreeRam,
      },
      // temporary hand-off: [0x2000, 0x2800)
      {
          .addr = 0x2000,
          .size = 0x0800,
          .type = memalloc::Type::kTemporaryPhysHandoff,
      },
      // free RAM: [0x2800, 0x3000)
      {
          .addr = 0x2800,
          .size = 0x0800,
          .type = memalloc::Type::kFreeRam,
      },
  };
  constexpr memalloc::Range kExpected[] = {
      {
          .addr = 0x0000,
          .size = 0x1000,
          .type = memalloc::Type::kPhysLog,
      },
      {
          .addr = 0x2000,
          .size = 0x1000,
          .type = memalloc::Type::kTemporaryPhysHandoff,
      },
  };
  ExpectAlignedAllocationsOrHoles({kInputRanges}, {kExpected});
}

TEST(ForEachAlignedAllocationOrHoleTests, HoleAndAllocationInSamePage) {
  // (allocation, hole) in page 1; RAM in page 2; (hole, allocation) in page 3
  constexpr memalloc::Range kInputRanges[] = {
      // phys log: [0, 0x800)
      {
          .addr = 0x0000,
          .size = 0x0800,
          .type = memalloc::Type::kPhysLog,
      },
      // hole: [0x800, 0x1000)
      // free RAM: [0x1000, 0x2000)
      {
          .addr = 0x1000,
          .size = 0x1000,
          .type = memalloc::Type::kFreeRam,
      },
      // hole: [0x2000, 0x2800)
      // temporary hand-off: [0x2800, 0x3000)
      {
          .addr = 0x2800,
          .size = 0x0800,
          .type = memalloc::Type::kTemporaryPhysHandoff,
      },
  };
  constexpr memalloc::Range kExpected[] = {
      {
          .addr = 0x0000,
          .size = 0x1000,
          .type = memalloc::Type::kPhysLog,
      },
      {
          .addr = 0x2000,
          .size = 0x1000,
          .type = memalloc::Type::kTemporaryPhysHandoff,
      },
  };
  ExpectAlignedAllocationsOrHoles({kInputRanges}, {kExpected});
}

}  // namespace
