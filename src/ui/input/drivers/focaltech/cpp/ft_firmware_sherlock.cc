// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/device-protocol/display-panel.h>

#include <array>

#include "ft_firmware.h"

namespace {

constexpr uint8_t kTopGroup9364Firmware[] = {
#include "prebuilt/touch/ft5726-sherlock/TopGroup_11mm_Fiti9364_0x0B.i"
};

constexpr uint8_t kLensOne9364Firmware[] = {
#include "prebuilt/touch/ft5726-sherlock/LensOne_11mm_Fiti9364_0x0D.i"
};

constexpr uint8_t kLensOne9365Firmware[] = {
#include "prebuilt/touch/ft5726-sherlock/LensOne_11mm_Fiti9365_0x0E.i"
};

}  // namespace

namespace ft {

const FirmwareEntry kFirmwareEntries[] = {
    {
        .panel_type = display::PanelType::kInnoluxP101dezFitipowerJd9364,
        .firmware_data = kTopGroup9364Firmware,
        .firmware_size = sizeof(kTopGroup9364Firmware),
    },
    {
        .panel_type = display::PanelType::kBoeTv101wxmFitipowerJd9364,
        .firmware_data = kLensOne9364Firmware,
        .firmware_size = sizeof(kLensOne9364Firmware),
    },
    {
        .panel_type = display::PanelType::kBoeTv101wxmFitipowerJd9365,
        .firmware_data = kLensOne9365Firmware,
        .firmware_size = sizeof(kLensOne9365Firmware),
    },
};

const size_t kNumFirmwareEntries = std::size(kFirmwareEntries);

}  // namespace ft
