// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FRAMEBUFFER_DISPLAY_FRAMEBUFFER_PCI_BOOT_DISPLAY_DRIVER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FRAMEBUFFER_DISPLAY_FRAMEBUFFER_PCI_BOOT_DISPLAY_DRIVER_H_

#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zbi-format/graphics.h>
#include <lib/zx/result.h>

#include <cstdint>
#include <string_view>

#include "src/graphics/display/lib/framebuffer-display/framebuffer-display-driver.h"
#include "src/graphics/display/lib/framebuffer-display/framebuffer-display.h"

namespace framebuffer_display {

// Base for PCI display drivers whose linear framebuffer was configured by the bootloader
// (BIOS/UEFI). Dimensions and pixel format of the framebuffer come from the ZBI framebuffer item
// (ZBI_TYPE_FRAMEBUFFER), populated by Gigaboot.
//
// Subclasses will supply the index of the PCI BAR that contains the framebuffer via
// `pci_bar_index`.
class FramebufferPciBootDisplayDriver : public FramebufferDisplayDriver {
 public:
  FramebufferPciBootDisplayDriver(std::string_view device_name, uint32_t pci_bar_index);

  FramebufferPciBootDisplayDriver(const FramebufferPciBootDisplayDriver&) = delete;
  FramebufferPciBootDisplayDriver(FramebufferPciBootDisplayDriver&&) = delete;
  FramebufferPciBootDisplayDriver& operator=(const FramebufferPciBootDisplayDriver&) = delete;
  FramebufferPciBootDisplayDriver& operator=(FramebufferPciBootDisplayDriver&&) = delete;

  ~FramebufferPciBootDisplayDriver() override;

  // FramebufferDisplayDriver:
  zx::result<> ConfigureHardware() override;
  zx::result<fdf::MmioBuffer> GetFrameBufferMmioBuffer() override;
  zx::result<DisplayProperties> GetDisplayProperties() override;

 private:
  zx::result<zbi_swfb_t> GetFramebufferInfo();

  const uint32_t pci_bar_index_;
};

}  // namespace framebuffer_display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_FRAMEBUFFER_DISPLAY_FRAMEBUFFER_PCI_BOOT_DISPLAY_DRIVER_H_
