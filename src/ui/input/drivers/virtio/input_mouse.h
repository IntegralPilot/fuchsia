// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_MOUSE_H_
#define SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_MOUSE_H_

#include <string>

#include "src/ui/input/drivers/virtio/input_device.h"

namespace virtio {

struct MouseReport {
  zx::time event_time = zx::time(ZX_TIME_INFINITE_PAST);

  enum ButtonIndex : uint8_t {
    kLeft = 1,
    kRight = 2,
    kMid = 3,

    kMaxButtonCount = 3,
  };
  std::array<bool, kMaxButtonCount> buttons = {false, false, false};
  int16_t rel_x;
  int16_t rel_y;
  int16_t rel_wheel;

  void ToFidlInputReport(
      fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
      fidl::AnyArena& allocator) const;
};

class HidMouse : public HidDevice<MouseReport> {
 public:
  HidMouse(std::string product_name, std::string serial_number)
      : product_name_(std::move(product_name)), serial_number_(std::move(serial_number)) {}

  fuchsia_input_report::wire::DeviceDescriptor GetDescriptor(fidl::AnyArena& allocator) override;
  void ReceiveEvent(virtio_input_event_t* event) override;

 private:
  void ReceiveRelEvent(virtio_input_event_t* event);
  void ReceiveKeyEvent(virtio_input_event_t* event);

  std::string product_name_;
  std::string serial_number_;
};

}  // namespace virtio

#endif  // SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_MOUSE_H_
