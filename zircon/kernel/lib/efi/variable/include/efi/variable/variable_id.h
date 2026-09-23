// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_EFI_STRING_INCLUDE_EFI_VARIABLE_ID_STRING_H_
#define ZIRCON_KERNEL_LIB_EFI_STRING_INCLUDE_EFI_VARIABLE_ID_STRING_H_

#include <efi/string/string.h>
#include <efi/types.h>
#include <fbl/vector.h>

namespace efi {

// "\0" is special constant for `EfiGetNextVariableName()` to start from the beginning.
// It is also used to indicate invalid value, since it is not allowed as variable name.
constexpr char16_t kInvalidVariableName[] = u"";

// UEFI variable id struct.
//
// UEFI uses variable name string and vendor ID to uniquely identify variable.
// This struct helps to deal with this pair as a separate type.
struct VariableId {
  efi::String name;
  efi_guid vendor_guid;

  // Checks if VariableId has valid EFI variable name.
  bool IsValid() const;

  // Marks VariableId as invalid.
  void Invalidate();
};

inline bool operator==(const VariableId& a, const VariableId& b) {
  return a.name == b.name && a.vendor_guid == b.vendor_guid;
}

inline bool operator!=(const VariableId& a, const VariableId& b) { return !(a == b); }

// Lt operator is provided for sorting purposes. User code shouldn't rely on any particular
// ordering.
bool operator<(const efi::VariableId& l, const efi::VariableId& r);

}  // namespace efi

#endif  // ZIRCON_KERNEL_LIB_EFI_STRING_INCLUDE_EFI_VARIABLE_ID_STRING_H_
