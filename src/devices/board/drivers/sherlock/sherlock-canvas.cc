// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/platform-defs.h>
#include <limits.h>

#include <sdk/lib/driver/component/cpp/composite_node_spec.h>
#include <sdk/lib/driver/component/cpp/node_add_args.h>
#include <soc/aml-t931/t931-hw.h>

#include "sherlock.h"

namespace sherlock {
namespace fpbus = fuchsia_hardware_platform_bus;

static const std::vector<fpbus::Mmio> sherlock_canvas_mmios{
    {{
        .base = T931_DMC_BASE,
        .length = T931_DMC_LENGTH,
    }},
};

static const std::vector<fpbus::Bti> sherlock_canvas_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_CANVAS,
    }},
};

static const fpbus::Node canvas_dev = []() {
  fpbus::Node dev = {};
  dev.name() = "canvas-ff638000";
  dev.vid() = PDEV_VID_AMLOGIC;
  dev.pid() = PDEV_PID_GENERIC;
  dev.did() = PDEV_DID_AMLOGIC_CANVAS;
  dev.mmio() = sherlock_canvas_mmios;
  dev.bti() = sherlock_canvas_btis;
  return dev;
}();

zx_status_t Sherlock::CanvasInit() {
  fidl::Arena<> fidl_arena;
  fdf::Arena arena('CANV');
  auto composite_spec =
      fuchsia_driver_framework::wire::CompositeNodeSpec::Builder(arena).name("aml-canvas").Build();

  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, canvas_dev),
                                                          composite_spec);
  if (!result.ok()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Canvas(canvas_dev) request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Canvas(canvas_dev) failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  return ZX_OK;
}

}  // namespace sherlock
