// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <soc/aml-s905d2/s905d2-hw.h>

#include "astro.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace astro {
namespace fpbus = fuchsia_hardware_platform_bus;

static const std::vector<fpbus::Mmio> astro_ram_ctl_mmios{
    {{
        .base = S905D2_DMC_BASE,
        .length = S905D2_DMC_LENGTH,
    }},
};

static const std::vector<fpbus::Bti> astro_ram_ctl_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_RAM_CTL,
    }},
};

static const std::vector<fpbus::Irq> astro_ram_ctl_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_DMC_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

static const fpbus::Node ramctl_dev = []() {
  fpbus::Node dev = {};
  dev.name() = "ram-controller-ff638000";
  dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
  dev.pid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D2;
  dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_RAM_CTL;
  dev.mmio() = astro_ram_ctl_mmios;
  dev.bti() = astro_ram_ctl_btis;
  dev.irq() = astro_ram_ctl_irqs;
  return dev;
}();

zx_status_t Astro::RamCtlInit() {
  fidl::Arena<> fidl_arena;
  fdf::Arena arena('RAMC');
  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, ramctl_dev),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "ram-controller-ff638000", .parents2 = {}}}));
  if (!result.ok()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec RamCtl(ramctl_dev) request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec RamCtl(ramctl_dev) failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  return ZX_OK;
}

}  // namespace astro
