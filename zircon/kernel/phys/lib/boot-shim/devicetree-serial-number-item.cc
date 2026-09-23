// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree.h>
#include <lib/devicetree/matcher.h>

namespace boot_shim {
namespace {

constexpr std::string_view kAndroidBootSerial = "androidboot.serialno=";

}  // namespace

devicetree::ScanState DevicetreeSerialNumberItem::OnNode(
    const devicetree::NodePath& path, const devicetree::PropertyDecoder& decoder) {
  if (path != "/") {
    return devicetree::ScanState::kDone;
  }

  auto serial_no = decoder.FindProperty("serial-number");
  if (!serial_no) {
    if (cmdline_.empty()) {
      return devicetree::ScanState::kDone;
    }

    size_t start_index = cmdline_.find(kAndroidBootSerial);
    if (start_index == std::string_view::npos) {
      return devicetree::ScanState::kDone;
    }
    std::string_view serial_number = cmdline_.substr(start_index + kAndroidBootSerial.size());
    serial_number = serial_number.substr(0, serial_number.find_first_of(' '));

    if (!serial_number.empty()) {
      set_payload({reinterpret_cast<const std::byte*>(serial_number.data()), serial_number.size()});
    }

    return devicetree::ScanState::kDone;
  }
  auto str = serial_no->AsString();
  if (str) {
    // Remove null terminator from the payload.
    set_payload({reinterpret_cast<const std::byte*>(str->data()), str->size()});
  }

  return devicetree::ScanState::kDone;
}

}  // namespace boot_shim
