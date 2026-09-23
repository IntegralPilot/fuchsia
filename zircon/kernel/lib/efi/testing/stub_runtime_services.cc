// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/efi/testing/stub_runtime_services.h>
#include <stdio.h>
#include <stdlib.h>

#include <algorithm>
#include <iterator>
#include <string_view>

#include <efi/variable/variable.h>
#include <fbl/vector.h>

namespace efi {

namespace {

using ::testing::_;
using ::testing::DoAll;
using ::testing::Return;
using ::testing::SetArgPointee;

// We need to stash the global StubRuntimeServices object here since there's
// no "self" parameter to any of these functions.
StubRuntimeServices* active_stub = nullptr;

// Wrapper to bounce the EFI C function pointer into our global StubRuntimeServices
// object.
template <auto func, typename... Args>
EFIAPI efi_status Wrap(Args... args) {
  if (!active_stub) {
    // Someone held onto the underlying function table after deleting
    // the parent StubRuntimeServices.
    return EFI_NOT_READY;
  }
  return (active_stub->*func)(args...);
}

}  // namespace

StubRuntimeServices::StubRuntimeServices()
    : services_{
          .GetTime = Wrap<&StubRuntimeServices::GetTime>,
          .SetTime = Wrap<&StubRuntimeServices::SetTime>,
          .GetWakeupTime = Wrap<&StubRuntimeServices::GetWakeupTime>,
          .SetWakeupTime = Wrap<&StubRuntimeServices::SetWakeupTime>,
          .SetVirtualAddressMap = Wrap<&StubRuntimeServices::SetVirtualAddressMap>,
          .ConvertPointer = Wrap<&StubRuntimeServices::ConvertPointer>,
          .GetVariable = Wrap<&StubRuntimeServices::GetVariable>,
          .GetNextVariableName = Wrap<&StubRuntimeServices::GetNextVariableName>,
          .SetVariable = Wrap<&StubRuntimeServices::SetVariable>,
          .GetNextHighMonotonicCount = Wrap<&StubRuntimeServices::GetNextHighMonotonicCount>,
          .ResetSystem = Wrap<&StubRuntimeServices::ResetSystem>,
          .UpdateCapsule = Wrap<&StubRuntimeServices::UpdateCapsule>,
          .QueryCapsuleCapabilities = Wrap<&StubRuntimeServices::QueryCapsuleCapabilities>,
          .QueryVariableInfo = Wrap<&StubRuntimeServices::QueryVariableInfo>,
      } {
  if (active_stub) {
    // We cannot support multiple StubRuntimeServices due to the global singleton
    // nature. Rather than causing hard-to-debug test behavior here, just fail
    // loudly and immediately.
    fprintf(stderr, "ERROR: cannot create multiple StubRuntimeService objects - exiting\n");
    exit(1);
  }
  active_stub = this;
}

StubRuntimeServices::~StubRuntimeServices() { active_stub = nullptr; }

void StubRuntimeServices::SetVariables(const std::list<Variable>& vars) {
  vars_ = vars;
  var_it_ = vars_->end();
}

void StubRuntimeServices::UnsetVariables() { vars_.reset(); }

// Safe length() implementation. Returns number of elements before `u'\0'` or `len`
inline size_t StrNLength(const char16_t* str, size_t len) {
  const char16_t* eof = std::char_traits<char16_t>::find(str, len, u'\0');
  return eof ? std::distance(str, eof) : len;
}

efi_status StubRuntimeServices::GetVariable(char16_t* var_name, efi_guid* vendor_guid,
                                            uint32_t* attributes, size_t* data_size, void* data) {
  if (!vars_)
    return EFI_UNSUPPORTED;

  auto it = std::find_if(vars_->begin(), vars_->end(), [&](const Variable& it) {
    return it.id == VariableId{String(var_name), *vendor_guid};
  });
  if (it == vars_->end())
    return EFI_NOT_FOUND;

  if (*data_size < it->value.size()) {
    *data_size = it->value.size();
    return EFI_BUFFER_TOO_SMALL;
  }

  std::copy_n(it->value.begin(), it->value.size(), (uint8_t*)data);
  *data_size = it->value.size();

  return EFI_SUCCESS;
}

efi_status StubRuntimeServices::GetNextVariableName(size_t* var_name_size, char16_t* var_name,
                                                    efi_guid* vendor_guid) {
  if (!vars_)
    return EFI_UNSUPPORTED;

  size_t str_len_safe = StrNLength(var_name, *var_name_size / sizeof(*var_name));
  efi::String var_name_str({var_name, str_len_safe});
  auto it = var_it_;
  if (var_name_str == efi::kInvalidVariableName) {
    // startover from the beginning
    var_it_ = vars_->begin();
    it = var_it_;
  } else {
    // If last value is passed increment tmp iterator
    if (it == vars_->end())
      return EFI_NOT_FOUND;

    if (it->id == VariableId{var_name_str, *vendor_guid})
      ++it;
  }

  if (it == vars_->end())
    return EFI_NOT_FOUND;

  const auto& it_var_name = std::u16string_view(it->id.name);
  const size_t it_var_name_size = (it_var_name.size() + 1) * sizeof(it_var_name[0]);
  if (it_var_name_size > *var_name_size) {
    *var_name_size = it_var_name_size;
    return EFI_BUFFER_TOO_SMALL;
  }

  std::copy_n(it_var_name.begin(), it_var_name.size(), var_name);
  var_name[it_var_name.size()] = u'\0';
  *vendor_guid = it->id.vendor_guid;
  *var_name_size = it_var_name_size;
  var_it_ = it;

  return EFI_SUCCESS;
}

efi_status StubRuntimeServices::QueryVariableInfo(uint32_t attributes,
                                                  uint64_t* max_var_storage_size,
                                                  uint64_t* remaining_var_storage_size,
                                                  uint64_t* max_var_size) {
  *max_var_storage_size = 1;
  *remaining_var_storage_size = 2;
  *max_var_size = 3;
  return EFI_SUCCESS;
}

}  // namespace efi
