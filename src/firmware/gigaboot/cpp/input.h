// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_GIGABOOT_CPP_INPUT_H_
#define SRC_FIRMWARE_GIGABOOT_CPP_INPUT_H_

#include <lib/zx/time.h>

#include <optional>
#include <string_view>

#include <efi/boot-services.h>
#include <efi/protocol/serial-io.h>
#include <efi/protocol/simple-text-output.h>
#include <efi/types.h>
#include <fbl/vector.h>

#include "utils.h"

namespace gigaboot {

// Wrapper class around console and serial input operations.
// Restores configurable attributes, e.g. serial poll frequency, cursor position,
// on destruction.
class InputReceiver {
 public:
  explicit InputReceiver(efi_system_table* sys) : sys_(sys) {}

  // Given a collection of valid characters, a timeout, and an optional prompt string,
  // return the first valid key pressed before the timeout expired,
  // or std::nullopt if no valid key was pressed before the end of the timeout.
  //
  // If the prompt string is not empty, it is printed once every second with a countdown
  // of the remaining timeout in seconds.
  std::optional<char> GetKeyPrompt(std::string_view valid_keys, zx::duration timeout,
                                   std::string_view prompt = "");

  // Get the next key pressed within the timeout period, or an error.
  fit::result<efi_status, char> GetKey(zx::duration timeout);

  efi_system_table* system_table() const { return sys_; }

 private:
  // Wrapper class for a serial input device.
  // Restores configurable attributes, e.g. serial poll frequency, on destruction.
  class Serial {
   public:
    ~Serial();

    // Set the serial poll frequency.
    // Note: SetPoll returns fit::ok() if there is no underlying serial device.
    fit::result<efi_status> SetPoll(zx::duration timeout);

    // Return the next character or an error.
    // Does not block; returns an error if there is no input within the timeout period.
    fit::result<efi_status, char> GetChar();

    explicit operator bool() const { return serial_.get(); }

   private:
    friend class InputReceiver;
    Serial();

    serial_io_mode mode_;
    EfiProtocolPtr<efi_serial_io_protocol> serial_;
  };

  friend class InputReceiverTest;
  Serial& serial() { return serial_; }

  efi_system_table* sys_;
  Serial serial_;
};

}  // namespace gigaboot

#endif  // SRC_FIRMWARE_GIGABOOT_CPP_INPUT_H_
