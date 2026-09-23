// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "input_touch.h"

#include <lib/ddk/debug.h>

#include <cinttypes>

#include <fbl/algorithm.h>

namespace virtio {

namespace {

constexpr fuchsia_input_report::wire::Axis CreateNonNegativeAxis(int64_t max_micrometer) {
  return {
      .range = {.min = 0, .max = max_micrometer},
      .unit =
          {
              .type = fuchsia_input_report::wire::UnitType::kMeters,
              .exponent = -6,
          },
  };
}

}  // namespace

void TouchReport::ToFidlInputReport(
    fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
    fidl::AnyArena& allocator) const {
  size_t count = 0;
  fidl::VectorView<fuchsia_input_report::wire::ContactInputReport> contact_rpt(allocator,
                                                                               kMaxTouchPoints);
  for (uint32_t i = 0; i < kMaxTouchPoints; i++) {
    if (!contacts[i].exists) {
      continue;
    }
    contact_rpt[count++] = fuchsia_input_report::wire::ContactInputReport::Builder(allocator)
                               .contact_id(i)
                               .position_x(contacts[i].x)
                               .position_y(contacts[i].y)
                               .Build();
  }
  contact_rpt.set_size(count);

  input_report.event_time(event_time.get())
      .touch(fuchsia_input_report::wire::TouchInputReport::Builder(allocator)
                 .contacts(contact_rpt)
                 .Build());
}

fuchsia_input_report::wire::DeviceDescriptor HidTouch::GetDescriptor(fidl::AnyArena& allocator) {
  auto device_info = fuchsia_input_report::wire::DeviceInformation::Builder(allocator);
  device_info.vendor_id(static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  device_info.product_id(
      static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kVirtioTouchscreen));

  if (!product_name_.empty()) {
    device_info.product_name(allocator, std::string_view(product_name_));
  }
  if (!serial_number_.empty()) {
    device_info.serial_number(allocator, std::string_view(serial_number_));
  }

  fidl::VectorView<fuchsia_input_report::wire::ContactInputDescriptor> contacts(allocator,
                                                                                kMaxTouchPoints);
  for (auto& c : contacts) {
    c = fuchsia_input_report::wire::ContactInputDescriptor::Builder(allocator)
            .position_x(CreateNonNegativeAxis(kXPhysicalMaxMicrometer))
            .position_y(CreateNonNegativeAxis(kYPhysicalMaxMicrometer))
            .Build();
  }

  const auto input = fuchsia_input_report::wire::TouchInputDescriptor::Builder(allocator)
                         .touch_type(fuchsia_input_report::wire::TouchType::kTouchscreen)
                         .max_contacts(kMaxTouchPoints)
                         .contacts(contacts)
                         .Build();

  const auto touch =
      fuchsia_input_report::wire::TouchDescriptor::Builder(allocator).input(input).Build();

  return fuchsia_input_report::wire::DeviceDescriptor::Builder(allocator)
      .device_information(device_info.Build())
      .touch(touch)
      .Build();
}

void HidTouch::ReceiveEvent(virtio_input_event_t* event) {
  if (event->type == VIRTIO_INPUT_EV_ABS) {
    if (event->code == VIRTIO_INPUT_EV_MT_SLOT) {
      if (event->value >= kMaxTouchPoints) {
        zxlogf(ERROR,
               "Touch input finger ID (%" PRIu32 ") exceeds the maximum finger ID supported %d",
               event->value, kMaxTouchPoints - 1);
        active_finger_index_ = -1;
        return;
      }
      active_finger_index_ = event->value;
    }

    if (active_finger_index_ < 0 || active_finger_index_ >= kMaxTouchPoints) {
      return;
    }

    if (event->code == VIRTIO_INPUT_EV_MT_TRACKING_ID) {
      // If tracking id is -1 we have to remove the finger from being tracked.
      report_.contacts[active_finger_index_].exists = static_cast<int32_t>(event->value) != -1;
    } else if (event->code == VIRTIO_INPUT_EV_MT_POSITION_X) {
      // By guaranteeing `event->value` <= max, the product will be <= kPhysicalMax, where both
      // kPhysicalMax and the product are int64_t, so we are guaranteed to not overflow.
      ZX_DEBUG_ASSERT(event->value <= x_info_.max);
      report_.contacts[active_finger_index_].x =
          event->value * kXPhysicalMaxMicrometer / x_info_.max;
    } else if (event->code == VIRTIO_INPUT_EV_MT_POSITION_Y) {
      // By guaranteeing `event->value` <= max, the product will be <= kPhysicalMax, where both
      // kPhysicalMax and the product are int64_t, so we are guaranteed to not overflow.
      ZX_DEBUG_ASSERT(event->value <= y_info_.max);
      report_.contacts[active_finger_index_].y =
          event->value * kYPhysicalMaxMicrometer / y_info_.max;
    }
  }
}

}  // namespace virtio
