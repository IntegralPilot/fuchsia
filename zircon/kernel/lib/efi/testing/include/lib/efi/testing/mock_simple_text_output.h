// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_EFI_TESTING_INCLUDE_LIB_EFI_TESTING_MOCK_SIMPLE_TEXT_OUTPUT_H_
#define ZIRCON_KERNEL_LIB_EFI_TESTING_INCLUDE_LIB_EFI_TESTING_MOCK_SIMPLE_TEXT_OUTPUT_H_

#include <efi/protocol/simple-text-output.h>
#include <gmock/gmock.h>

#include "mock_protocol_base.h"

namespace efi {
class MockSimpleTextOutputProtocol
    : public MockProtocolBase<MockSimpleTextOutputProtocol, efi_simple_text_output_protocol> {
 public:
  explicit MockSimpleTextOutputProtocol(bool visibile)
      : MockProtocolBase({
            .Reset = Bounce<&MockSimpleTextOutputProtocol::Reset>,
            .SetCursorPosition = Bounce<&MockSimpleTextOutputProtocol::SetCursorPosition>,
            .EnableCursor = Bounce<&MockSimpleTextOutputProtocol::EnableCursor>,
        }) {
    mode_ = {
        .CursorVisible = visibile,
    };
    wrapper_.Mode = &mode_;
  }

  MOCK_METHOD(efi_status, Reset, (bool extended_verification));
  MOCK_METHOD(efi_status, EnableCursor, (bool visible));
  MOCK_METHOD(efi_status, SetCursorPosition, (size_t col, size_t row));

 private:
  simple_text_output_mode mode_;
};
}  // namespace efi

#endif  // ZIRCON_KERNEL_LIB_EFI_TESTING_INCLUDE_LIB_EFI_TESTING_MOCK_SIMPLE_TEXT_OUTPUT_H_
