// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_SKELETON_UNIT_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_SKELETON_UNIT_H_

namespace zxdb {

// Holds the relevant info for the a DW_TAG_skeleton_unit for the index.
//
// When using Debug Fission, each source file will have its own symbols stored in a separate .dwo
// file. The main binary references the .dwo files with a "skeleton unit" in the main binary that
// stores the information on how to locate the .dwo file and how to relocate its addresses.
struct SkeletonUnit {
  // Offset in the main binary of the DIE defining the skeleton unit. This is used later to
  // reconstruct the symbol if we need it (normally it's not needed).
  uint64_t skeleton_die_offset = 0;

  std::string dwo_name;
  std::string comp_dir;

  // Byte offset into the .debug_addr section of the main binary containing the .dwo file's
  // relocations.
  //
  // This is how the link-independent .dwo files are updated for the way the code is linked
  // together. The DIEs in the DWO file references indices into the address table for the location
  // of the code the symbols refer to. This table is stored in the main binary and updated by the
  // linker. This addr_base is the offset within the address data of the main binary that the
  // corresponing DWO's address table starts at. So all DWO address indices are relative to this
  // offset.
  uint64_t addr_base = 0;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_SKELETON_UNIT_H_
