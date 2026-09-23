// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_DTS_VIM3_IOMMU_H_
#define SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_DTS_VIM3_IOMMU_H_

// Add new iommu id at the end of this list.
#define DWMAC_BTI 0x1
#define VDEC_BTI 0x2
#define AUDIO_BTI 0x3
#define DWC2_BTI 0x4
#define USB_PHY_BTI 0x5
#define XHCI_BTI 0x6
#define EMMC_A_BTI 0x7
#define EMMC_B_BTI 0x8
#define EMMC_C_BTI 0x9
#define MALI_BTI 0xA
#define NNA_BTI 0xB
#define CANVAS_BTI 0xC
#define DISPLAY_BTI 0xD
#define SYSMEM_BTI 0xE

#endif  // SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_DTS_VIM3_IOMMU_H_