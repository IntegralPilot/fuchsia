// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_GIGABOOT_CPP_TESTS_MOCK_EFI_VARIABLES_H_
#define SRC_FIRMWARE_GIGABOOT_CPP_TESTS_MOCK_EFI_VARIABLES_H_

#include <gmock/gmock.h>

#include "efi_variables.h"

namespace gigaboot {

class MockEfiVariables : public EfiVariables {
 public:
  MOCK_METHOD((fit::result<efi_status, EfiVariableInfo>), EfiQueryVariableInfo, (),
              (const, override));
  MOCK_METHOD((fit::result<efi_status, fbl::Vector<uint8_t>>), EfiGetVariable,
              (const efi::VariableId&), (const, override));
  MOCK_METHOD((fit::result<efi_status, efi_guid>), GetGuid, (std::u16string_view var_name),
              (override));
  MOCK_METHOD(fit::result<efi_status>, EfiGetNextVariableName, (efi::VariableId&),
              (const, override));
};

}  // namespace gigaboot

#endif  // SRC_FIRMWARE_GIGABOOT_CPP_TESTS_MOCK_EFI_VARIABLES_H_
