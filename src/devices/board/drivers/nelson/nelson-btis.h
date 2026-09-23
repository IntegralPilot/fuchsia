// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_DRIVERS_NELSON_NELSON_BTIS_H_
#define SRC_DEVICES_BOARD_DRIVERS_NELSON_NELSON_BTIS_H_

namespace nelson {

// BTI IDs for our devices
enum {
  BTI_BOARD,
  BTI_USB,
  BTI_DISPLAY,
  BTI_EMMC,
  BTI_MALI,
  BTI_VIDEO,
  BTI_SDIO,
  BTI_CANVAS,
  BTI_AUDIO_IN,
  BTI_AUDIO_OUT,
  BTI_TEE,
  BTI_SYSMEM,
  BTI_AML_SECURE_MEM,
  BTI_NNA,
  BTI_AUDIO_BT_IN,
  BTI_AUDIO_BT_OUT,
  BTI_SPI1,
};

}  // namespace nelson

#endif  // SRC_DEVICES_BOARD_DRIVERS_NELSON_NELSON_BTIS_H_
