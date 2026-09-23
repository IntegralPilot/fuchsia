// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/input/drivers/virtio/input_mouse.h"

#include <lib/ddk/debug.h>
#include <zircon/status.h>

#include <fbl/algorithm.h>
#include <virtio/input.h>

namespace virtio {

namespace {

constexpr uint16_t kKeyCodeBtnLeft = 0x110;    // BTN_LEFT
constexpr uint16_t kKeyCodeBtnRight = 0x111;   // BTN_RIGHT
constexpr uint16_t kKeyCodeBtnMiddle = 0x112;  // BTN_MIDDLE

constexpr int64_t kRangeMin = -32767;
constexpr int64_t kRangeMax = 32767;
constexpr fuchsia_input_report::wire::Axis kMouseRange = {
    .range = {.min = kRangeMin, .max = kRangeMax},
    .unit =
        {
            .type = fuchsia_input_report::wire::UnitType::kNone,
            .exponent = 0,
        },
};

}  // namespace

void MouseReport::ToFidlInputReport(
    fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
    fidl::AnyArena& allocator) const {
  fidl::VectorView<uint8_t> pressed_buttons(allocator, kMaxButtonCount);
  size_t idx = 0;
  for (uint8_t i = 0; i < kMaxButtonCount; i++) {
    if (buttons[i]) {
      pressed_buttons[idx++] = i + 1;
    }
  }
  pressed_buttons.set_size(idx);

  auto mouse_rpt = fuchsia_input_report::wire::MouseInputReport::Builder(allocator);
  mouse_rpt.pressed_buttons(pressed_buttons);
  mouse_rpt.movement_x(rel_x);
  mouse_rpt.movement_y(rel_y);
  mouse_rpt.scroll_v(rel_wheel);

  input_report.event_time(event_time.get()).mouse(mouse_rpt.Build());
}

fuchsia_input_report::wire::DeviceDescriptor HidMouse::GetDescriptor(fidl::AnyArena& allocator) {
  auto device_info = fuchsia_input_report::wire::DeviceInformation::Builder(allocator);
  device_info.vendor_id(static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  device_info.product_id(
      static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kVirtioMouse));

  if (!product_name_.empty()) {
    device_info.product_name(allocator, std::string_view(product_name_));
  }
  if (!serial_number_.empty()) {
    device_info.serial_number(allocator, std::string_view(serial_number_));
  }

  const auto input =
      fuchsia_input_report::wire::MouseInputDescriptor::Builder(allocator)
          .buttons({allocator,
                    {MouseReport::ButtonIndex::kLeft, MouseReport::ButtonIndex::kRight,
                     MouseReport::ButtonIndex::kMid}})
          .movement_x(kMouseRange)
          .movement_y(kMouseRange)
          .scroll_v(kMouseRange)
          .Build();

  const auto mouse =
      fuchsia_input_report::wire::MouseDescriptor::Builder(allocator).input(input).Build();

  return fuchsia_input_report::wire::DeviceDescriptor::Builder(allocator)
      .device_information(device_info.Build())
      .mouse(mouse)
      .Build();
}

void HidMouse::ReceiveKeyEvent(virtio_input_event_t* event) {
  ZX_DEBUG_ASSERT(event->type == VIRTIO_INPUT_EV_KEY);
  uint16_t key_code = event->code;
  uint32_t status = event->value;

  uint8_t button_idx = 0;
  switch (key_code) {
    case kKeyCodeBtnLeft:
      button_idx = MouseReport::ButtonIndex::kLeft;
      break;
    case kKeyCodeBtnRight:
      button_idx = MouseReport::ButtonIndex::kRight;
      break;
    case kKeyCodeBtnMiddle:
      button_idx = MouseReport::ButtonIndex::kMid;
      break;
    default:
      zxlogf(ERROR, "%s: key code %u not supported!", __func__, key_code);
      return;
  }

  report_.buttons[button_idx - 1] = status == VIRTIO_INPUT_EV_KEY_PRESSED;
}

void HidMouse::ReceiveRelEvent(virtio_input_event_t* event) {
  ZX_DEBUG_ASSERT(event->type == VIRTIO_INPUT_EV_REL);
  switch (event->code) {
    case VIRTIO_INPUT_EV_REL_X:
      report_.rel_x = static_cast<int16_t>(event->value);
      break;
    case VIRTIO_INPUT_EV_REL_Y:
      report_.rel_y = static_cast<int16_t>(event->value);
      break;
    case VIRTIO_INPUT_EV_REL_HWHEEL:
      // TODO(65215): Support horizontal wheel scrolling.
      break;
    case VIRTIO_INPUT_EV_REL_WHEEL:
      report_.rel_wheel = static_cast<int16_t>(event->value);
      break;
    default:
      zxlogf(ERROR, "%s: event code %u not supported!", __func__, event->code);
      return;
  }
}

void HidMouse::ReceiveEvent(virtio_input_event_t* event) {
  switch (event->type) {
    case VIRTIO_INPUT_EV_KEY:
      ReceiveKeyEvent(event);
      break;
    case VIRTIO_INPUT_EV_REL:
      ReceiveRelEvent(event);
      break;
    case VIRTIO_INPUT_EV_SYN:
      // EV_SYN events will be handled by InputDevice directly after calling
      // |ReceiveEvent|, so we ignore the SYN event here.
      break;
    default:
      zxlogf(ERROR, "%s: unsupported event type %u!", __func__, event->type);
      break;
  }
}

}  // namespace virtio
