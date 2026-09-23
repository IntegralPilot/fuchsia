// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_DRIVERS_SHERLOCK_SHERLOCK_BTIS_H_
#define SRC_DEVICES_BOARD_DRIVERS_SHERLOCK_SHERLOCK_BTIS_H_

namespace sherlock {

// BTI IDs for our devices
enum {
  BTI_BOARD,
  BTI_USB,
  BTI_EMMC,
  BTI_SDIO,
  BTI_MALI,
  BTI_CANVAS,
  BTI_VIDEO,
  BTI_ISP,
  BTI_MIPI,
  BTI_GDC,
  BTI_DISPLAY,
  BTI_AUDIO_OUT,
  BTI_AUDIO_IN,
  BTI_AUDIO_BT_OUT,
  BTI_AUDIO_BT_IN,
  BTI_SYSMEM,
  BTI_TEE,
  BTI_GE2D,
  BTI_NNA,
  BTI_AML_SECURE_MEM,
  BTI_VIDEO_ENC,
  BTI_HEVC_ENC,
};

}  // namespace sherlock

#endif  // SRC_DEVICES_BOARD_DRIVERS_SHERLOCK_SHERLOCK_BTIS_H_
