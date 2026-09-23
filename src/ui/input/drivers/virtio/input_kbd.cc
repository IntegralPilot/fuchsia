// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/input/drivers/virtio/input_kbd.h"

#include <lib/ddk/debug.h>
#include <zircon/status.h>

#include <fbl/algorithm.h>

#include "src/devices/bus/lib/virtio/trace.h"

#define LOCAL_TRACE 0

namespace virtio {

namespace {

// These are Linux input event codes:
// https://github.com/torvalds/linux/blob/HEAD/include/uapi/linux/input-event-codes.h
constexpr std::optional<fuchsia_input::wire::Key> kEventCodeMap[] = {
    /* 0x00 */ std::nullopt,
    /* 0x01 */ fuchsia_input::wire::Key::kEscape,
    /* 0x02 */ fuchsia_input::wire::Key::kKey1,
    /* 0x03 */ fuchsia_input::wire::Key::kKey2,
    /* 0x04 */ fuchsia_input::wire::Key::kKey3,
    /* 0x05 */ fuchsia_input::wire::Key::kKey4,
    /* 0x06 */ fuchsia_input::wire::Key::kKey5,
    /* 0x07 */ fuchsia_input::wire::Key::kKey6,
    /* 0x08 */ fuchsia_input::wire::Key::kKey7,
    /* 0x09 */ fuchsia_input::wire::Key::kKey8,
    /* 0x0a */ fuchsia_input::wire::Key::kKey9,
    /* 0x0b */ fuchsia_input::wire::Key::kKey0,
    /* 0x0c */ fuchsia_input::wire::Key::kMinus,
    /* 0x0d */ fuchsia_input::wire::Key::kEquals,
    /* 0x0e */ fuchsia_input::wire::Key::kBackspace,
    /* 0x0f */ fuchsia_input::wire::Key::kTab,
    /* 0x10 */ fuchsia_input::wire::Key::kQ,
    /* 0x11 */ fuchsia_input::wire::Key::kW,
    /* 0x12 */ fuchsia_input::wire::Key::kE,
    /* 0x13 */ fuchsia_input::wire::Key::kR,
    /* 0x14 */ fuchsia_input::wire::Key::kT,
    /* 0x15 */ fuchsia_input::wire::Key::kY,
    /* 0x16 */ fuchsia_input::wire::Key::kU,
    /* 0x17 */ fuchsia_input::wire::Key::kI,
    /* 0x18 */ fuchsia_input::wire::Key::kO,
    /* 0x19 */ fuchsia_input::wire::Key::kP,
    /* 0x1a */ fuchsia_input::wire::Key::kLeftBrace,
    /* 0x1b */ fuchsia_input::wire::Key::kRightBrace,
    /* 0x1c */ fuchsia_input::wire::Key::kEnter,
    /* 0x1d */ fuchsia_input::wire::Key::kLeftCtrl,
    /* 0x1e */ fuchsia_input::wire::Key::kA,
    /* 0x1f */ fuchsia_input::wire::Key::kS,
    /* 0x20 */ fuchsia_input::wire::Key::kD,
    /* 0x21 */ fuchsia_input::wire::Key::kF,
    /* 0x22 */ fuchsia_input::wire::Key::kG,
    /* 0x23 */ fuchsia_input::wire::Key::kH,
    /* 0x24 */ fuchsia_input::wire::Key::kJ,
    /* 0x25 */ fuchsia_input::wire::Key::kK,
    /* 0x26 */ fuchsia_input::wire::Key::kL,
    /* 0x27 */ fuchsia_input::wire::Key::kSemicolon,
    /* 0x28 */ fuchsia_input::wire::Key::kApostrophe,
    /* 0x29 */ fuchsia_input::wire::Key::kGraveAccent,
    /* 0x2a */ fuchsia_input::wire::Key::kLeftShift,
    /* 0x2b */ fuchsia_input::wire::Key::kBackslash,
    /* 0x2c */ fuchsia_input::wire::Key::kZ,
    /* 0x2d */ fuchsia_input::wire::Key::kX,
    /* 0x2e */ fuchsia_input::wire::Key::kC,
    /* 0x2f */ fuchsia_input::wire::Key::kV,
    /* 0x30 */ fuchsia_input::wire::Key::kB,
    /* 0x31 */ fuchsia_input::wire::Key::kN,
    /* 0x32 */ fuchsia_input::wire::Key::kM,
    /* 0x33 */ fuchsia_input::wire::Key::kComma,
    /* 0x34 */ fuchsia_input::wire::Key::kDot,
    /* 0x35 */ fuchsia_input::wire::Key::kSlash,
    /* 0x36 */ fuchsia_input::wire::Key::kRightShift,
    /* 0x37 */ fuchsia_input::wire::Key::kKeypadAsterisk,
    /* 0x38 */ fuchsia_input::wire::Key::kLeftAlt,
    /* 0x39 */ fuchsia_input::wire::Key::kSpace,
    /* 0x3a */ fuchsia_input::wire::Key::kCapsLock,
    /* 0x3b */ fuchsia_input::wire::Key::kF1,
    /* 0x3c */ fuchsia_input::wire::Key::kF2,
    /* 0x3d */ fuchsia_input::wire::Key::kF3,
    /* 0x3e */ fuchsia_input::wire::Key::kF4,
    /* 0x3f */ fuchsia_input::wire::Key::kF5,
    /* 0x40 */ fuchsia_input::wire::Key::kF6,
    /* 0x41 */ fuchsia_input::wire::Key::kF7,
    /* 0x42 */ fuchsia_input::wire::Key::kF8,
    /* 0x43 */ fuchsia_input::wire::Key::kF9,
    /* 0x44 */ fuchsia_input::wire::Key::kF10,
    /* 0x45 */ fuchsia_input::wire::Key::kNumLock,
    /* 0x46 */ fuchsia_input::wire::Key::kScrollLock,
    /* 0x47 */ fuchsia_input::wire::Key::kKeypad7,
    /* 0x48 */ fuchsia_input::wire::Key::kKeypad8,
    /* 0x49 */ fuchsia_input::wire::Key::kKeypad9,
    /* 0x4a */ fuchsia_input::wire::Key::kKeypadMinus,
    /* 0x4b */ fuchsia_input::wire::Key::kKeypad4,
    /* 0x4c */ fuchsia_input::wire::Key::kKeypad5,
    /* 0x4d */ fuchsia_input::wire::Key::kKeypad6,
    /* 0x4e */ fuchsia_input::wire::Key::kKeypadPlus,
    /* 0x4f */ fuchsia_input::wire::Key::kKeypad1,
    /* 0x50 */ fuchsia_input::wire::Key::kKeypad2,
    /* 0x51 */ fuchsia_input::wire::Key::kKeypad3,
    /* 0x52 */ fuchsia_input::wire::Key::kKeypad0,
    /* 0x53 */ fuchsia_input::wire::Key::kKeypadDot,
    /* 0x54 */ std::nullopt,
    /* 0x55 */ std::nullopt,
    /* 0x56 */ std::nullopt,
    /* 0x57 */ fuchsia_input::wire::Key::kF11,
    /* 0x58 */ fuchsia_input::wire::Key::kF12,
    /* 0x59 */ std::nullopt,
    /* 0x5a */ std::nullopt,
    /* 0x5b */ std::nullopt,
    /* 0x5c */ std::nullopt,
    /* 0x5d */ std::nullopt,
    /* 0x5e */ std::nullopt,
    /* 0x5f */ std::nullopt,
    /* 0x60 */ std::nullopt,
    /* 0x61 */ fuchsia_input::wire::Key::kRightCtrl,
    /* 0x62 */ std::nullopt,
    /* 0x63 */ std::nullopt,
    /* 0x64 */ fuchsia_input::wire::Key::kRightAlt,
    /* 0x65 */ std::nullopt,
    /* 0x66 */ fuchsia_input::wire::Key::kHome,
    /* 0x67 */ fuchsia_input::wire::Key::kUp,
    /* 0x68 */ fuchsia_input::wire::Key::kPageUp,
    /* 0x69 */ fuchsia_input::wire::Key::kLeft,
    /* 0x6a */ fuchsia_input::wire::Key::kRight,
    /* 0x6b */ fuchsia_input::wire::Key::kEnd,
    /* 0x6c */ fuchsia_input::wire::Key::kDown,
    /* 0x6d */ fuchsia_input::wire::Key::kPageDown,
    /* 0x6e */ fuchsia_input::wire::Key::kInsert,
    /* 0x6f */ fuchsia_input::wire::Key::kDelete,
    /* 0x70 */ std::nullopt,
    /* 0x71 */ fuchsia_input::wire::Key::kMute,
    /* 0x72 */ fuchsia_input::wire::Key::kVolumeDown,
    /* 0x73 */ fuchsia_input::wire::Key::kVolumeUp,
    /* 0x74 */ fuchsia_input::wire::Key::kPower,
    /* 0x75 */ std::nullopt,
    /* 0x76 */ std::nullopt,
    /* 0x77 */ fuchsia_input::wire::Key::kPause,
    /* 0x78 */ std::nullopt,
    /* 0x79 */ std::nullopt,
    /* 0x7a */ std::nullopt,
    /* 0x7b */ std::nullopt,
    /* 0x7c */ std::nullopt,
    /* 0x7d */ fuchsia_input::wire::Key::kLeftMeta,
    /* 0x7e */ fuchsia_input::wire::Key::kRightMeta,
};

constexpr size_t kKeyCount = []() {
  size_t count = 0;
  for (const auto& k : kEventCodeMap) {
    if (k.has_value()) {
      count++;
    }
  }
  return count;
}();

constexpr std::array<fuchsia_input::wire::Key, kKeyCount> kKeys = []() {
  std::array<fuchsia_input::wire::Key, kKeyCount> keys;
  size_t i = 0;
  for (const auto& k : kEventCodeMap) {
    if (k.has_value()) {
      keys[i++] = k.value();
    }
  }
  return keys;
}();

}  // namespace

void KeyboardReport::ToFidlInputReport(
    fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
    fidl::AnyArena& allocator) const {
  fidl::VectorView<fuchsia_input::wire::Key> keys3(allocator, kMaxKeys);
  size_t idx = 0;
  for (const auto& key : usage) {
    if (!key.has_value()) {
      break;
    }
    keys3[idx++] = *key;
  }
  keys3.set_size(idx);

  auto keyboard_report =
      fuchsia_input_report::wire::KeyboardInputReport::Builder(allocator).pressed_keys3(keys3);
  input_report.event_time(event_time.get()).keyboard(keyboard_report.Build());
}

fuchsia_input_report::wire::DeviceDescriptor HidKeyboard::GetDescriptor(fidl::AnyArena& allocator) {
  auto device_info = fuchsia_input_report::wire::DeviceInformation::Builder(allocator);
  device_info.vendor_id(static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  device_info.product_id(
      static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kVirtioKeyboard));

  if (!product_name_.empty()) {
    device_info.product_name(allocator, std::string_view(product_name_));
  }
  if (!serial_number_.empty()) {
    device_info.serial_number(allocator, std::string_view(serial_number_));
  }

  const auto input =
      fuchsia_input_report::wire::KeyboardInputDescriptor::Builder(allocator).keys3(kKeys).Build();

  const auto keyboard =
      fuchsia_input_report::wire::KeyboardDescriptor::Builder(allocator).input(input).Build();

  return fuchsia_input_report::wire::DeviceDescriptor::Builder(allocator)
      .device_information(device_info.Build())
      .keyboard(keyboard)
      .Build();
}

void HidKeyboard::AddKeypressToReport(uint16_t event_code) {
  auto hid_code = kEventCodeMap[event_code];
  if (!hid_code.has_value()) {
    return;
  }
  for (auto& usage : report_.usage) {
    if (!usage.has_value()) {
      usage = hid_code;
      return;
    }
    if (*usage == *hid_code) {
      // The key already exists in the report so we ignore it.
      return;
    }
  }

  // There's no free slot in the report.
  // TODO: Record a rollover status.
}

void HidKeyboard::RemoveKeypressFromReport(uint16_t event_code) {
  auto hid_code = kEventCodeMap[event_code];
  if (!hid_code.has_value()) {
    return;
  }
  int id = -1;
  for (int i = 0; i != kMaxKeys; ++i) {
    if (report_.usage[i].has_value() && *report_.usage[i] == *hid_code) {
      id = i;
      break;
    }
  }

  if (id == -1) {
    // They key is not in the report so we ignore it.
    return;
  }

  for (size_t i = id; i != kMaxKeys - 1; ++i) {
    report_.usage[i] = report_.usage[i + 1];
  }
  report_.usage[kMaxKeys - 1] = std::nullopt;
}

void HidKeyboard::ReceiveEvent(virtio_input_event_t* event) {
  if (event->type != VIRTIO_INPUT_EV_KEY) {
    zxlogf(TRACE, "Unsupported event type %d\n", event->type);
    return;
  }
  if (event->code == 0 || event->code >= std::size(kEventCodeMap)) {
    zxlogf(TRACE, "Unknown key %d\n", event->code);
    return;
  }
  if (event->value == VIRTIO_INPUT_EV_KEY_PRESSED) {
    AddKeypressToReport(event->code);
  } else if (event->value == VIRTIO_INPUT_EV_KEY_RELEASED) {
    RemoveKeypressFromReport(event->code);
  }
}

}  // namespace virtio
