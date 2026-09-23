// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/hid/buttons.h>
#include <lib/hid/descriptor.h>

// clang-format off
static const uint8_t buttons_report_desc[] = {
    HID_USAGE_PAGE(0x0C), // Consumer
    HID_USAGE(0x01), // Consumer Control
    HID_COLLECTION_APPLICATION,

    HID_REPORT_ID(BUTTONS_RPT_ID_INPUT),

    HID_USAGE_PAGE(0x0C), // Consumer
    HID_USAGE(0xE9), // Volume Up
    HID_USAGE(0xEA), // Volume Down
    HID_USAGE(0x31), // Reset
    HID_USAGE(0x77), // Camera access disabled
    HID_LOGICAL_MIN(0),
    HID_LOGICAL_MAX(1),
    HID_REPORT_SIZE(1),
    HID_REPORT_COUNT(4),
    HID_INPUT(HID_Data_Var_Abs),
    HID_REPORT_SIZE(4), // Padding
    HID_REPORT_COUNT(1),
    HID_INPUT(HID_Const_Arr_Abs),

    HID_USAGE_PAGE(0x0B), // Telephony
    HID_USAGE(0x2F), // Mute microphone
    HID_LOGICAL_MIN(0),
    HID_LOGICAL_MAX(1),
    HID_REPORT_SIZE(1),
    HID_REPORT_COUNT(1),
    HID_INPUT(HID_Data_Var_Abs),
    HID_REPORT_SIZE(7), // Padding
    HID_REPORT_COUNT(1),
    HID_INPUT(HID_Const_Arr_Abs),

    HID_END_COLLECTION,
};
// clang-format on

size_t get_buttons_report_desc(const uint8_t** buf) {
  *buf = buttons_report_desc;
  return sizeof(buttons_report_desc);
}

void fill_button_in_report(fuchsia_buttons::GpioButtonId id, bool value, buttons_input_rpt_t* rpt) {
  switch (id) {
    case fuchsia_buttons::GpioButtonId::kVolumeUp:
      if (value) {
        rpt->volume_up = 1;
      }
      break;
    case fuchsia_buttons::GpioButtonId::kVolumeDown:
      if (value) {
        rpt->volume_down = 1;
      }
      break;
    case fuchsia_buttons::GpioButtonId::kFdr:
      if (value) {
        rpt->reset = 1;
      }
      break;
    case fuchsia_buttons::GpioButtonId::kMicMute:
      rpt->mute = value;
      break;
    case fuchsia_buttons::GpioButtonId::kCamMute:
      rpt->camera_access_disabled = value;
      break;
    case fuchsia_buttons::GpioButtonId::kMicAndCamMute:
      rpt->camera_access_disabled = value;
      rpt->mute = value;
      break;
    default:
      break;
  }
}
