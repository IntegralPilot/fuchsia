// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LIB_MEMALLOC_INCLUDE_LIB_MEMALLOC_TESTING_COMPARE_H_
#define ZIRCON_KERNEL_PHYS_LIB_MEMALLOC_INCLUDE_LIB_MEMALLOC_TESTING_COMPARE_H_

#include <lib/memalloc/range.h>
#include <stdio.h>

#include <algorithm>
#include <span>
#include <sstream>
#include <string>

#include <gtest/gtest.h>

namespace memalloc {

// Enables Range auto-stringification in gtest error messages
inline std::ostream& operator<<(std::ostream& stream, Range range) {
  constexpr uint64_t kMax = std::numeric_limits<uint64_t>::max();
  stream << ToString(range.type) << ": ";
  if (range.size == 0) {
    stream << "Ø";
  } else {
    stream << "[" << std::hex << range.addr << ", " << std::hex
           << (range.addr + std::min(kMax - range.addr, range.size)) << ")";
  }
  return stream;
}

namespace testing {

inline std::string ToString(const memalloc::Range& range) {
  std::stringstream ss;
  ss << range;
  return ss.str();
}

template <typename RangeIter>
inline std::string ToString(RangeIter first, RangeIter last) {
  std::string s;
  for (auto it = first; it != last; ++it) {
    s += "  " + ToString(*it) + "\n";
  }
  return s;
}

template <typename Ranges>
inline std::string ToString(Ranges&& ranges) {
  return ToString(ranges.begin(), ranges.end());
}

inline std::string ToString(const std::span<const memalloc::Range>& ranges) {
  return ToString(ranges.begin(), ranges.end());
}

inline void CompareRanges(std::span<const memalloc::Range> expected,
                          std::span<const memalloc::Range> actual) {
  EXPECT_EQ(expected.size(), actual.size());
  size_t num_comparable = std::min(expected.size(), actual.size());
  for (size_t i = 0; i < num_comparable; ++i) {
    EXPECT_EQ(expected[i], actual[i]) << i;
  }

  if (expected.size() > num_comparable) {
    printf("Unaccounted for expected ranges:\n%s",
           ToString(expected.begin() + num_comparable, expected.end()).c_str());
  }
  if (actual.size() > num_comparable) {
    printf("Unaccounted for actual ranges:\n%s",
           ToString(actual.begin() + num_comparable, actual.end()).c_str());
  }
}

}  // namespace testing
}  // namespace memalloc

#endif  // ZIRCON_KERNEL_PHYS_LIB_MEMALLOC_INCLUDE_LIB_MEMALLOC_TESTING_COMPARE_H_
