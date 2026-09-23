// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <string_view>

#include <efi/variable/variable_id.h>

namespace efi {

bool VariableId::IsValid() const { return name != kInvalidVariableName; }

// Marks iterator as invalid in case of error.
void VariableId::Invalidate() { name = efi::String(kInvalidVariableName); }

bool operator<(const efi::VariableId& l, const efi::VariableId& r) {
  auto cmp = std::string_view(l.name).compare(std::string_view(r.name));
  if (cmp == 0) {
    return l.vendor_guid < r.vendor_guid;
  } else if (cmp < 0) {
    return true;
  } else {
    return false;
  }
}

}  // namespace efi
