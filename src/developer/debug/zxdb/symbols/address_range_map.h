// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_ADDRESS_RANGE_MAP_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_ADDRESS_RANGE_MAP_H_

#include <iostream>
#include <vector>

#include "gtest/gtest_prod.h"
#include "src/developer/debug/shared/largest_less_or_equal.h"
#include "src/developer/debug/zxdb/common/string_util.h"

namespace zxdb {

// A constant mapping of ranges to a value (DIE offset). This is stored in a flat sorted structure.
// Holes and the end of the last range are indicated by "null" values.
class AddressRangeMap {
 public:
  // Define the "mapped-to" types and the value to use to denote null. This is specified here
  // because in the future we may want to make this class a template and these would be the template
  // parameters.
  //
  // In the current implementation the Value is a DIE offset.
  using Value = uint64_t;
  static constexpr Value kNullValue = 0;

  AddressRangeMap() {}

  bool empty() const { return map_.empty(); }

  // Returns kNullValue if not found.
  Value Lookup(uint64_t addr) const;

  // Debug dumping.
  void Dump(std::ostream& out) const;

 private:
  friend class AddressRangeMapBuilder;
  FRIEND_TEST(AddressRangeMap, Query);
  FRIEND_TEST(AddressRangeMapBuilder, Build);

  struct Record {
    Record(uint64_t b, Value v) : begin(b), value(v) {}

    // For unit tests.
    bool operator==(const Record& other) const {
      return begin == other.begin && value == other.value;
    }

    uint64_t begin;
    Value value;
  };

  explicit AddressRangeMap(std::vector<Record> input) : map_(std::move(input)) {}

  // A flat map holding the beginning of the range and the corresponding value for that range. Empty
  // ranges are denoted with kNullValue. The last element should also be kNullValue to indicate the
  // end of the last valid range.
  std::vector<Record> map_;
};

inline AddressRangeMap::Value AddressRangeMap::Lookup(uint64_t addr) const {
  auto found = debug::LargestLessOrEqual(
      map_.begin(), map_.end(), Record(addr, kNullValue),
      [](const Record& a, const Record& b) { return a.begin < b.begin; },
      [](const Record& a, const Record& b) { return a.begin == b.begin; });
  if (found == map_.end()) {
    return kNullValue;
  }
  return found->value;
}

inline void AddressRangeMap::Dump(std::ostream& out) const {
  out << "AddressRangeMap:\n";
  for (const auto& r : map_) {
    out << "  " << to_hex_string(r.begin, 8) << ": " << to_hex_string(r.value) << "\n";
  }
}

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_ADDRESS_RANGE_MAP_H_
