// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_ADDRESS_RANGE_MAP_BUILDER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_ADDRESS_RANGE_MAP_BUILDER_H_

#include <map>

#include "src/developer/debug/zxdb/common/address_ranges.h"
#include "src/developer/debug/zxdb/symbols/address_range_map.h"

namespace zxdb {

// A builder for an AddressRangeMap. It requires a more complex structure to construct than the
// final flattened one, and this object encapsulates that.
class AddressRangeMapBuilder {
 public:
  // This assumes the input ranges are from DWARF and so are hierarchical and ranges are added from
  // larger to smaller. If a new range crosses multiple existing ranges, for example, the result
  // will be incorrect.
  void AddRange(const AddressRange& range, AddressRangeMap::Value die_offset);
  void AddRanges(const AddressRanges& ranges, AddressRangeMap::Value die_offset);

  AddressRangeMap GetMap() const;

 private:
  std::map<AddressRange, AddressRangeMap::Value, debug::AddressRangeBeginCmp> map_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_ADDRESS_RANGE_MAP_BUILDER_H_
