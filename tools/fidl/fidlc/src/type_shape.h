// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_TYPE_SHAPE_H_
#define TOOLS_FIDL_FIDLC_SRC_TYPE_SHAPE_H_

#include <cstdint>

namespace fidlc {

// A type shape describes the wire layout of a type.
// Apart from has_flexible_envelope, it does not take unknown envelopes into account.
// For example, an empty table has max_handles=0 even though unknown envelopes can have handles.
struct TypeShape {
  // Inline size of the type in bytes. Always a multiple of the alignment.
  uint32_t inline_size = 0;
  // Alignment of the type in bytes.
  uint32_t alignment = 1;
  // Maximum depth, or UINT32_MAX if unbounded.
  // Each pointer or envelope (even if inline) increments the depth.
  uint32_t depth = 0;
  // Maximum number of handles, or UINT32_MAX if unbounded.
  uint32_t max_handles = 0;
  // Maximum out-of-line size in bytes (always a multiple of 8), or UINT32_MAX if unbounded.
  uint32_t max_out_of_line = 0;
  // True if the type has inline or out-of-line padding.
  bool has_padding = false;
  // True if the type may contain an unknown envelope, i.e. contains a table or flexible union.
  bool has_flexible_envelope = false;
};

// A field shape describes the wire layout of a field within a struct.
// The struct's inline_size is equal to its last field's offset + inline_size + padding.
struct FieldShape {
  // Offset from the start of the struct in bytes.
  uint32_t offset = 0;
  // Padding after the field's inline size in bytes, to align the next field.
  // For the last field, this pads the struct to be a multiple of its alignment.
  uint32_t padding = 0;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_TYPE_SHAPE_H_
