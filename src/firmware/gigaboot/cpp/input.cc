// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "input.h"

#include <inttypes.h>
#include <limits.h>
#include <log.h>

#include <algorithm>
#include <memory>

#include "efi/types.h"
#include "lib/fit/internal/result.h"
#include "lib/fit/result.h"
#include "utils.h"

namespace gigaboot {

namespace {

// Super basic single-character UTF-16 to ASCII conversion. Anything outside of
// the [0x01, 0x7F] range just gets converted to std::nullopt.
std::optional<char> SimpleUtf16ToAscii(uint16_t utf16) {
  return (utf16 & 0xFF80 || !utf16) ? std::nullopt : std::optional(static_cast<char>(utf16 & 0x7F));
}

fit::result<efi_status, char> GetAsciiKeyConIn(efi_system_table* sys) {
  efi_input_key key;
  efi_status res = sys->ConIn->ReadKeyStroke(sys->ConIn, &key);
  if (res != EFI_SUCCESS) {
    return fit::error(res);
  }

  std::optional<char> c = SimpleUtf16ToAscii(key.UnicodeChar);
  if (c) {
    return fit::ok(*c);
  }

  return fit::error(EFI_UNSUPPORTED);
}

class Cursor {
 public:
  Cursor() = delete;
  Cursor(const Cursor&) = delete;
  Cursor(Cursor&&) = delete;
  Cursor& operator=(const Cursor&) = delete;
  Cursor& operator=(Cursor&&) = delete;

  Cursor(efi_system_table* sys, simple_text_output_mode mode) : sys_(sys), mode_(mode) {}
  ~Cursor() {
    ResetPosition();
    SetVisibility(mode_.CursorVisible);
  }

  // Explicitly not static because we want to force users to go through the constructor as a guard.
  // Don't care about the error code either.
  void SetVisibility(bool on) const { sys_->ConOut->EnableCursor(sys_->ConOut, on); }

  void ResetPosition() const {
    sys_->ConOut->SetCursorPosition(sys_->ConOut, mode_.CursorColumn, mode_.CursorRow);
  }

 private:
  efi_system_table* sys_;
  simple_text_output_mode mode_;
};

}  // namespace

InputReceiver::Serial::Serial() {
  auto res = EfiLocateProtocol<efi_serial_io_protocol>();
  if (res.is_ok()) {
    serial_ = std::move(*res);
    if (serial_->Mode) {
      mode_ = *serial_->Mode;
    }
  }
}

InputReceiver::Serial::~Serial() {
  if (serial_) {
    efi_status res =
        serial_->SetAttributes(serial_.get(), mode_.BaudRate, mode_.ReceiveFifoDepth, mode_.Timeout,
                               mode_.Parity, static_cast<uint8_t>(mode_.DataBits), mode_.StopBits);
    if (res != EFI_SUCCESS) {
      printf("failed to restore serial attributes: %s\n", EfiStatusToString(res));
    }
  }
}

fit::result<efi_status> InputReceiver::Serial::SetPoll(zx::duration timeout) {
  efi_status res = EFI_SUCCESS;
  if (serial_) {
    res = serial_->SetAttributes(serial_.get(), mode_.BaudRate, mode_.ReceiveFifoDepth,
                                 static_cast<uint32_t>(timeout.to_usecs()), mode_.Parity,
                                 static_cast<uint8_t>(mode_.DataBits), mode_.StopBits);
  }

  // If there is no serial, SetPoll is a no-op.
  // This makes sense from a caller perspective:
  // they don't actually care whether there is a serial device, only that
  // if there were one it would behave appropriately.
  if (res == EFI_SUCCESS) {
    return fit::ok();
  }

  return fit::error(res);
}

fit::result<efi_status, char> InputReceiver::Serial::GetChar() {
  // GetChar is different from SetPoll: an error isn't necessarily unrecoverable,
  // it may mean that the poll just timed out.
  if (!serial_) {
    return fit::error(EFI_UNSUPPORTED);
  }

  char c;
  size_t read_len = 1;
  efi_status status = serial_->Read(serial_.get(), &read_len, &c);
  if (status == EFI_SUCCESS && read_len == 1 && c != '\0') {
    return fit::ok(c);
  }

  return fit::error((status == EFI_SUCCESS) ? EFI_NOT_READY : status);
}

fit::result<efi_status, char> InputReceiver::GetKey(zx::duration timeout) {
  fit::result<efi_status> res = serial_.SetPoll(zx::msec(1));
  if (res.is_error()) {
    return res.take_error();
  }

  Timer timer(sys_);
  res = timer.SetTimer(TimerRelative, timeout);
  if (res.is_error()) {
    printf("%s: failed to set timer: %s\n", __func__, EfiStatusToString(res.error_value()));
    return res.take_error();
  }

  // Run the checks at least once so we poll if timeout == 0.
  do {
    // Console input gets priority, check it first.
    fit::result<efi_status, char> c = GetAsciiKeyConIn(sys_);
    if (c.is_ok()) {
      return c;
    }

    c = serial_.GetChar();
    if (c.is_ok()) {
      return c;
    }

  } while (timer.CheckTimer() == Timer::Status::kWaiting);

  return fit::error(EFI_TIMEOUT);
}

std::optional<char> InputReceiver::GetKeyPrompt(std::string_view valid_keys, zx::duration timeout,
                                                std::string_view prompt) {
  if (valid_keys.empty()) {
    return std::nullopt;
  }

  Cursor cursor(sys_, (sys_->ConOut->Mode) ? *sys_->ConOut->Mode : simple_text_output_mode());
  cursor.SetVisibility(false);

  const zx::duration tick = zx::sec(timeout == zx::duration::infinite() ? 0 : 1);
  do {
    if (!prompt.empty()) {
      if (tick != zx::duration(0)) {
        LOG("%.*s %" PRIu64 "s", static_cast<int>(prompt.size()), prompt.data(), timeout.to_secs());
      } else {
        LOG("%.*s", static_cast<int>(prompt.size()), prompt.data());
      }
    }

    fit::result<efi_status, char> key = GetKey(zx::sec(1));
    if (key.is_ok() &&
        std::find(valid_keys.cbegin(), valid_keys.cend(), *key) != valid_keys.cend()) {
      return *key;
    }

    cursor.ResetPosition();
    timeout -= tick;
  } while (timeout > zx::sec(0));

  return std::nullopt;
}

}  // namespace gigaboot
