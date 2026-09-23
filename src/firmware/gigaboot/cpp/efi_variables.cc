// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "efi_variables.h"

#include <inttypes.h>
#include <stdio.h>
#include <xefi.h>

#include <algorithm>
#include <string>
#include <string_view>

#include <efi/variable/variable_id.h>
#include <fbl/vector.h>
#include <phys/efi/main.h>

namespace fbl {
template <typename T>
inline bool operator==(const fbl::Vector<T>& a, const fbl::Vector<T>& b) {
  return std::equal(a.begin(), a.end(), b.begin(), b.end());
}

template <typename T>
inline bool operator!=(const fbl::Vector<T>& a, const fbl::Vector<T>& b) {
  return !(a == b);
}
}  // namespace fbl

namespace gigaboot {

fit::result<efi_status, EfiVariables::EfiVariableInfo> EfiVariables::EfiQueryVariableInfo() const {
  efi_status status;
  EfiVariableInfo efi_var_info = {};

  status = gEfiSystemTable->RuntimeServices->QueryVariableInfo(
      EFI_VARIABLE_BOOTSERVICE_ACCESS | EFI_VARIABLE_RUNTIME_ACCESS,
      &efi_var_info.max_var_storage_size, &efi_var_info.remaining_var_storage_size,
      &efi_var_info.max_var_size);
  if (EFI_ERROR(status)) {
    return fit::error(status);
  }

  return fit::success(efi_var_info);
}

fit::result<efi_status> EfiVariables::EfiGetNextVariableName(efi::VariableId& variable_id) const {
  auto [name_utf8, name_ucs2] = variable_id.name.TakeData();
  for (size_t i = 0; i < 2; i++) {
    // Get next variable name
    size_t variable_name_size = name_ucs2.size() * sizeof(name_ucs2[0]);
    efi_status status = gEfiSystemTable->RuntimeServices->GetNextVariableName(
        &variable_name_size, name_ucs2.data(), &variable_id.vendor_guid);
    if (EFI_BUFFER_TOO_SMALL == status) {
      name_ucs2.resize(variable_name_size / sizeof(name_ucs2[0]));
      continue;
    }
    if (EFI_ERROR(status)) {
      variable_id.name = efi::String(std::move(name_utf8));
      return fit::error(status);
    }

    name_ucs2.resize(variable_name_size / sizeof(name_ucs2[0]));
    variable_id.name = efi::String(std::move(name_ucs2));
    return fit::ok();
  }

  variable_id.name = efi::String(std::move(name_utf8));
  return fit::error(EFI_ABORTED);
}

fit::result<efi_status, efi::VariableValue> EfiVariables::EfiGetVariable(
    const efi::VariableId& variable_id) const {
  efi::VariableValue res;

  // Get variable length first
  size_t variable_size = 0;
  efi_guid vendor_guid = variable_id.vendor_guid;
  std::u16string_view name_ucs2 = variable_id.name;
  efi_status status = gEfiSystemTable->RuntimeServices->GetVariable(
      const_cast<char16_t*>(name_ucs2.data()), &vendor_guid, NULL, &variable_size, nullptr);

  // Get buffer of the correct size and read into it
  if (EFI_BUFFER_TOO_SMALL == status) {
    res.resize(variable_size);

    status = gEfiSystemTable->RuntimeServices->GetVariable(
        const_cast<char16_t*>(name_ucs2.data()), &vendor_guid, NULL, &variable_size, res.data());
  }

  if (EFI_ERROR(status)) {
    return fit::error(status);
  }

  return fit::success(std::move(res));
}

fit::result<efi_status, efi_guid> EfiVariables::GetGuid(std::u16string_view var_name) {
  auto IsNameMatch = [&var_name](const efi::VariableId& variable_id) {
    return variable_id.name == var_name;
  };

  auto res = std::find_if(begin(), end(), IsNameMatch);
  if (res == end()) {
    if (res.has_failed_) {
      return fit::error(EFI_ABORTED);
    } else {
      return fit::error(EFI_NOT_FOUND);
    }
  }

  // Check if there is another variable with the same name
  auto res2 = res;
  res2 = std::find_if(std::next(res2), end(), IsNameMatch);
  if (res2 != end()) {
    return fit::error(EFI_INVALID_PARAMETER);
  }

  return fit::success(res.variable_id_.vendor_guid);
}

bool EfiVariables::iterator::operator==(const iterator& other) const {
  return (is_end_ == other.is_end_) && (is_end_ || variable_id_ == other.variable_id_);
}

bool EfiVariables::iterator::operator!=(const iterator& other) const { return !(*this == other); }

EfiVariables::iterator& EfiVariables::iterator::operator++() {  // prefix
  if (is_end_) {
    return *this;
  }

  if (has_failed_) {
    is_end_ = true;
    return *this;
  }

  auto variable_id = container_->EfiGetNextVariableName(variable_id_);
  if (variable_id.is_error()) {
    if (variable_id.error_value() == EFI_NOT_FOUND) {
      is_end_ = true;
    } else {
      has_failed_ = true;
      variable_id_.Invalidate();
      printf("Failed to get variable name from EfiGetNextVariableName(): %s",
             xefi_strerror(variable_id.error_value()));
    }
  }
  return *this;
}

EfiVariables::iterator EfiVariables::iterator::operator++(int) {  // postfix
  iterator old = *this;
  ++*this;
  return old;
}

bool EfiVariables::EfiVariableInfo::operator==(
    const EfiVariables::EfiVariableInfo& other) const noexcept {
  return max_var_storage_size == other.max_var_storage_size &&
         remaining_var_storage_size == other.remaining_var_storage_size &&
         max_var_size == other.max_var_size;
}

bool EfiVariables::EfiVariableInfo::operator!=(const EfiVariableInfo& other) const noexcept {
  return !(*this == other);
}

fbl::Vector<char16_t> ToVector(std::u16string_view str) {
  fbl::Vector<char16_t> res;
  res.resize(str.size());
  std::copy(str.begin(), str.end(), res.begin());
  return res;
}

}  // namespace gigaboot
