// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <inttypes.h>

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_UNIT_INDEX_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_UNIT_INDEX_H_

namespace zxdb {

// Holds a way to find a unit in a DWARF binary. DWARF stores the compilation unit information for
// binaries and .dwo files in different ELF sections. So if we want to reference a unit, we need to
// know both the index and which section to look in.
struct UnitIndex {
  UnitIndex() = default;
  UnitIndex(bool d, uint32_t i) : is_dwo(d), index(i) {}

  // When set, this means that the binary is a .dwo file and this index is into the ELF section for
  // Debug Fission data.
  //
  // This is not to be confused with a "skeleton unit" which is the unit in a normal DWARF binary
  // that references the .dwo file.
  bool is_dwo = false;

  uint32_t index = 0;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_UNIT_INDEX_H_
