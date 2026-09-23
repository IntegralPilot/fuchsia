// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_KBD_H_
#define SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_KBD_H_

#include <string>

#include "input_device.h"

namespace virtio {

static constexpr int kMaxKeys = 6;
struct KeyboardReport {
  zx::time event_time = zx::time(ZX_TIME_INFINITE_PAST);
  std::array<std::optional<fuchsia_input::wire::Key>, kMaxKeys> usage;

  void ToFidlInputReport(
      fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
      fidl::AnyArena& allocator) const;
};

class HidKeyboard : public HidDevice<KeyboardReport> {
 public:
  HidKeyboard(std::string product_name, std::string serial_number)
      : product_name_(std::move(product_name)), serial_number_(std::move(serial_number)) {}

  fuchsia_input_report::wire::DeviceDescriptor GetDescriptor(fidl::AnyArena& allocator) override;
  void ReceiveEvent(virtio_input_event_t* event) override;

 private:
  void AddKeypressToReport(uint16_t event_code);
  void RemoveKeypressFromReport(uint16_t event_code);

  std::string product_name_;
  std::string serial_number_;
};

}  // namespace virtio

#endif  // SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_KBD_H_
