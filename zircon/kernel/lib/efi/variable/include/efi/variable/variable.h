// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_EFI_STRING_INCLUDE_EFI_VARIABLE_STRING_H_
#define ZIRCON_KERNEL_LIB_EFI_STRING_INCLUDE_EFI_VARIABLE_STRING_H_

#include <efi/types.h>
#include <efi/variable/variable_id.h>
#include <fbl/vector.h>

namespace efi {

// Variable value type alias
using VariableValue = fbl::Vector<uint8_t>;

// Creates a duplicate of VariableValue.
VariableValue Copy(const VariableValue& source);

// UEFI variable representation: Id and Value.
struct Variable {
  VariableId id;
  VariableValue value;

  Variable(const VariableId& id, const VariableValue& value);
  Variable(const Variable& source);
  Variable(Variable&& source) noexcept = default;
  Variable& operator=(const Variable& source) noexcept;
  Variable& operator=(Variable&& source) noexcept = default;
};

}  // namespace efi

#endif  // ZIRCON_KERNEL_LIB_EFI_STRING_INCLUDE_EFI_VARIABLE_STRING_H_
