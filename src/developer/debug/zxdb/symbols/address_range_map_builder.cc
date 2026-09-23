// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/address_range_map_builder.h"

#include "src/developer/debug/shared/largest_less_or_equal.h"

namespace zxdb {

void AddressRangeMapBuilder::AddRange(const AddressRange& range,
                                      AddressRangeMap::Value die_offset) {
  if (map_.empty()) {
    map_.emplace(range, die_offset);
    return;
  }

  // The first existing range beginning at or after the new one (computes >=).
  auto lower = map_.lower_bound(range);

  // We actually want to find the LAST range less than or equal to.
  if (lower == map_.end()) {
    // Past the beginning of the last range.
    --lower;
  } else if (lower->first.begin() > range.begin()) {
    // Past the beginning of the found range.
    if (lower == map_.begin()) {
      // Input is before the first existing range, just insert it.
      map_.emplace(range, die_offset);
      return;
    }
    --lower;
  }

  AddressRange found_range = lower->first;  // Need copy because we will invalidate "lower" below.
  if (range.begin() >= found_range.end()) {
    // Input is past the end, just insert it.
    map_.emplace(range, die_offset);
    return;
  }

  // If we get here, the existing range needs splitting.
  AddressRangeMap::Value found_value = lower->second;

  // Save the insert position before deleting the existing range. |lower| IS INVALIDATED.
  auto insert_hint = lower;
  ++insert_hint;
  map_.erase(lower);

  // Any range before the new one gets the existing value.
  if (found_range.begin() < range.begin()) {
    map_.emplace_hint(insert_hint, AddressRange(found_range.begin(), range.begin()), found_value);
  }

  // Always followed by the new range.
  map_.emplace_hint(insert_hint, range, die_offset);

  // Any range after the new one gets the existing value.
  if (found_range.end() > range.end()) {
    map_.emplace_hint(insert_hint, AddressRange(range.end(), found_range.end()), found_value);
  }
}

void AddressRangeMapBuilder::AddRanges(const AddressRanges& ranges,
                                       AddressRangeMap::Value die_offset) {
  for (const auto& r : ranges) {
    AddRange(r, die_offset);
  }
}

AddressRangeMap AddressRangeMapBuilder::GetMap() const {
  if (map_.empty()) {
    return AddressRangeMap();
  }

  std::vector<AddressRangeMap::Record> output;
  // Reserve extra space because there will be gaps.
  output.reserve(map_.size() + map_.size() / 2);

  // Flatten the map.
  uint64_t prev_end = std::numeric_limits<uint64_t>::max();
  for (auto [range, value] : map_) {
    if (range.begin() > prev_end) {
      // Mark the blank space between the previous range and this one.
      output.emplace_back(prev_end, AddressRangeMap::kNullValue);
    }
    output.emplace_back(range.begin(), value);
    prev_end = range.end();
  }

  // Mark the end of the last range.
  output.emplace_back(prev_end, AddressRangeMap::kNullValue);

  return AddressRangeMap(std::move(output));
}

}  // namespace zxdb
