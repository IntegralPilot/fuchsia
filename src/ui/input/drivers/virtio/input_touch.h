// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_TOUCH_H_
#define SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_TOUCH_H_

#include <string>

#include "input_device.h"

namespace virtio {

static constexpr int kMaxTouchPoints = 5;
struct TouchReport {
  zx::time event_time = zx::time(ZX_TIME_INFINITE_PAST);
  struct Contact {
    bool exists = false;
    int64_t x = 0;
    int64_t y = 0;
  };
  std::array<Contact, kMaxTouchPoints> contacts = {};

  void ToFidlInputReport(
      fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
      fidl::AnyArena& allocator) const;
};

// The HidTouch class translates virtio touchscreen events into HID touchscreen events. It does this
// by making the virtio touchscreen appear exactly like a Google Pixelbook (Eve) touchscreen. There
// is no good reason to use the Google Pixelbook (Eve) touchscreen, other than it is a valid, tested
// report descriptor and it was easier to reuse it than building a new report descriptor from
// scratch.
class HidTouch : public HidDevice<TouchReport> {
 public:
  HidTouch(virtio_input_absinfo_t x_info, virtio_input_absinfo_t y_info, std::string product_name,
           std::string serial_number)
      : x_info_(x_info),
        y_info_(y_info),
        product_name_(std::move(product_name)),
        serial_number_(std::move(serial_number)) {}

  fuchsia_input_report::wire::DeviceDescriptor GetDescriptor(fidl::AnyArena& allocator) override;
  void ReceiveEvent(virtio_input_event_t* event) override;

  // These numbers are used by the touch descriptor of the Google Pixelbook (Eve) touchscreen. We
  // use these physical sizes to emulate a real touch device on the emulator.
  static constexpr int64_t kXPhysicalMaxMicrometer = 259200;
  static constexpr int64_t kYPhysicalMaxMicrometer = 172800;

 private:
  virtio_input_absinfo_t x_info_;
  virtio_input_absinfo_t y_info_;
  // The index of the current finger that the driver is processing.
  // Must be [0..kMaxTouchPoints - 1] for a valid finger or -1 when no finger is being processed.
  int active_finger_index_ = -1;

  std::string product_name_;
  std::string serial_number_;
};

}  // namespace virtio

#endif  // SRC_UI_INPUT_DRIVERS_VIRTIO_INPUT_TOUCH_H_
