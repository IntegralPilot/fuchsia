// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_THERMAL_CPP_INCLUDE_LIB_THERMAL_LINEAR_LOOKUP_TABLE_H_
#define SRC_DEVICES_LIB_THERMAL_CPP_INCLUDE_LIB_THERMAL_LINEAR_LOOKUP_TABLE_H_

#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <algorithm>
#include <vector>

namespace linear_lookup_table {

template <typename xType, typename yType>
struct LookupTableEntry {
  xType x;
  yType y;
};

// Both x and y axis must be monotonicly decreasing/increasing.
template <typename xType, typename yType>
class LinearLookupTable {
 private:
  using LookupTableEntryType = LookupTableEntry<xType, yType>;

 public:
  explicit LinearLookupTable(std::vector<LookupTableEntryType> table)
      : table_([&table]() {
          // Sort profile table descending by x to ensure proper lookup
          auto sort_compare = [](LookupTableEntryType const& lhs,
                                 LookupTableEntryType const& rhs) -> bool { return lhs.x > rhs.x; };
          std::sort(table.begin(), table.end(), sort_compare);
          return std::move(table);
        }()),
        y_decreasing_((table_.size() <= 1) || (table_[0].y > table_[1].y)) {}

  explicit LinearLookupTable(std::vector<xType> x_list, std::vector<yType> y_list)
      : LinearLookupTable([&x_list, &y_list]() {
          ZX_ASSERT(x_list.size() == y_list.size());
          std::vector<LookupTableEntryType> table;
          std::transform(x_list.begin(), x_list.end(), y_list.begin(), std::back_inserter(table),
                         [](xType a, yType b) { return LookupTableEntryType{a, b}; });
          return std::move(table);
        }()) {}

  // Always returns float because it may be linearly interpolated. x-axis is always decreasing.
  zx::result<float> LookupY(float x) const { return Lookup(x, kYAxis, true); }

  // Always returns float because it may be linearly interpolated.
  zx::result<float> LookupX(float y) const { return Lookup(y, kXAxis, y_decreasing_); }

 private:
  struct axis_t {
    std::function<float(const LookupTableEntryType& entry)> lookup;
    std::function<float(const LookupTableEntryType& entry)> reference;
  };

  axis_t kXAxis = {
      .lookup = [](const LookupTableEntryType& entry) { return static_cast<float>(entry.x); },
      .reference = [](const LookupTableEntryType& entry) { return static_cast<float>(entry.y); },
  };

  axis_t kYAxis = {
      .lookup = [](const LookupTableEntryType& entry) { return static_cast<float>(entry.y); },
      .reference = [](const LookupTableEntryType& entry) { return static_cast<float>(entry.x); },
  };

  zx::result<float> Lookup(float reference_value, axis_t axis, bool decreasing) const {
    if (table_.empty()) {
      return zx::error(ZX_ERR_NO_RESOURCES);
    }

    size_t min_idx = decreasing ? 0 : table_.size() - 1;
    size_t max_idx = decreasing ? table_.size() - 1 : 0;
    if (reference_value >= axis.reference(table_[min_idx])) {
      return zx::ok(axis.lookup(table_[min_idx]));
    }
    if (reference_value <= axis.reference(table_[max_idx])) {
      return zx::ok(axis.lookup(table_[max_idx]));
    }

    auto lb_compare = [&axis, &decreasing](LookupTableEntryType const& lhs, float val) -> bool {
      return decreasing ? (axis.reference(lhs) > val) : (axis.reference(lhs) < val);
    };
    auto low = std::lower_bound(table_.begin(), table_.end(), reference_value, lb_compare);
    size_t idx = (low - table_.begin());

    return zx::ok(LinearInterpolate(axis.reference(table_.at(idx)), axis.lookup(table_.at(idx)),
                                    axis.reference(table_.at(idx - 1)),
                                    axis.lookup(table_.at(idx - 1)), reference_value));
  }

  // Find y for (x, y) on the line of (x1, y1) (x2, y2)
  static constexpr float LinearInterpolate(float x1, float y1, float x2, float y2, float x) {
    float scale = (x - x1) / (x2 - x1);
    return scale * (y2 - y1) + y1;
  }

  std::vector<LookupTableEntryType> table_;
  bool y_decreasing_;
};
}  // namespace linear_lookup_table

#endif  // SRC_DEVICES_LIB_THERMAL_CPP_INCLUDE_LIB_THERMAL_LINEAR_LOOKUP_TABLE_H_
