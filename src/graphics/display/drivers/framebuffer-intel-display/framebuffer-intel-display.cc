// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_export2.h>

#include "src/graphics/display/lib/framebuffer-display/framebuffer-pci-boot-display-driver.h"

namespace framebuffer_display {

namespace {

constexpr uint32_t kFramebufferPciBarIndex = 2;

class FramebufferIntelDisplayDriver final : public FramebufferPciBootDisplayDriver {
 public:
  FramebufferIntelDisplayDriver();

  FramebufferIntelDisplayDriver(const FramebufferIntelDisplayDriver&) = delete;
  FramebufferIntelDisplayDriver(FramebufferIntelDisplayDriver&&) = delete;
  FramebufferIntelDisplayDriver& operator=(const FramebufferIntelDisplayDriver&) = delete;
  FramebufferIntelDisplayDriver& operator=(FramebufferIntelDisplayDriver&&) = delete;

  ~FramebufferIntelDisplayDriver() override;
};

FramebufferIntelDisplayDriver::FramebufferIntelDisplayDriver()
    : FramebufferPciBootDisplayDriver("framebuffer-intel-display", kFramebufferPciBarIndex) {}

FramebufferIntelDisplayDriver::~FramebufferIntelDisplayDriver() = default;

}  // namespace

}  // namespace framebuffer_display

FUCHSIA_DRIVER_EXPORT2(framebuffer_display::FramebufferIntelDisplayDriver);
