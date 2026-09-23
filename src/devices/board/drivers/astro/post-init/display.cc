// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <bind/fuchsia/amlogic/platform/s905d2/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <soc/aml-s905d2/s905d2-gpio.h>
#include <soc/aml-s905d2/s905d2-hw.h>

#include "src/devices/board/drivers/astro/astro-btis.h"
#include "src/devices/board/drivers/astro/post-init/post-init.h"

namespace astro {
namespace fpbus = fuchsia_hardware_platform_bus;

static const std::vector<fpbus::Mmio> display_mmios{
    {{
        // VPU
        .base = S905D2_VPU_BASE,
        .length = S905D2_VPU_LENGTH,
        .name = "vpu",
    }},
    {{
        // MIPI DSI "TOP"
        .base = S905D2_MIPI_TOP_DSI_BASE,
        .length = S905D2_MIPI_TOP_DSI_LENGTH,
        .name = "dsi-top",
    }},
    {{
        // MIPI DSI PHY
        .base = S905D2_DSI_PHY_BASE,
        .length = S905D2_DSI_PHY_LENGTH,
        .name = "dsi-phy",
    }},
    {{
        // DSI Host Controller
        .base = S905D2_MIPI_DSI_BASE,
        .length = S905D2_MIPI_DSI_LENGTH,
        .name = "dsi-controller",
    }},
    {{
        // HIU / HHI
        .base = S905D2_HIU_BASE,
        .length = S905D2_HIU_LENGTH,
        .name = "hhi",
    }},
    {{
        // AOBUS
        // TODO(https://fxbug.dev/42081392): Restrict range to RTI
        .base = S905D2_AOBUS_BASE,
        .length = 0x1000,
        .name = "always-on-rti",
    }},
    {{
        // RESET
        .base = S905D2_RESET_BASE,
        .length = S905D2_RESET_LENGTH,
        .name = "ee-reset",
    }},
    {{
        // PERIPHS_REGS (GPIO Multiplexer)
        .base = S905D2_GPIO_BASE,
        .length = S905D2_GPIO_LENGTH,
        .name = "gpio-mux",
    }},
};

static const std::vector<fpbus::Irq> display_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_VIU1_VSYNC_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
        .name = "viu1-vsync",
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_RDMA_DONE),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
        .name = "rdma-done",
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_VID1_WR),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
        .name = "vdin1-write-done",
    }},
};

static const std::vector<fpbus::Bti> display_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_DISPLAY,
    }},
};

zx::result<> PostInit::InitDisplay() {
  const std::vector<fpbus::Metadata> display_panel_metadata{
      {{
          .id = std::to_string(DEVICE_METADATA_DISPLAY_PANEL_TYPE),
          .data = std::vector(reinterpret_cast<uint8_t*>(&panel_type_),
                              reinterpret_cast<uint8_t*>(&panel_type_) + sizeof(panel_type_)),
      }},
  };

  std::string display_name;
  if (panel_type_ == display::PanelType::kBoeTv070wsmFitipowerJd9364Astro) {
    display_name = "boe-display-ff900000";
  } else if (panel_type_ == display::PanelType::kInnoluxP070acbFitipowerJd9364) {
    display_name = "innolux-display-ff900000";
  } else {
    fdf::error("Unknown panel type: {}", static_cast<uint32_t>(panel_type_));
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const fpbus::Node display_dev = [&]() {
    fpbus::Node dev = {};
    dev.name() = display_name;
    dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
    dev.pid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D2;
    dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_DISPLAY;
    dev.metadata() = display_panel_metadata;
    dev.mmio() = display_mmios;
    dev.irq() = display_irqs;
    dev.bti() = display_btis;
    return dev;
  }();

  std::vector<fuchsia_driver_framework::BindRule2> gpio_bind_rules{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_s905d2::GPIOH_PIN_ID_PIN_6),
  };

  std::vector<fuchsia_driver_framework::NodeProperty2> gpio_properties{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-lcd-reset"),
  };

  std::vector<fuchsia_driver_framework::BindRule2> canvas_bind_rules{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.amlogiccanvas.Service"),
  };

  std::vector<fuchsia_driver_framework::NodeProperty2> canvas_properties{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.amlogiccanvas.Service"),
  };

  std::vector<fuchsia_driver_framework::ParentSpec2> parents = {
      {{
          .bind_rules = gpio_bind_rules,
          .properties = gpio_properties,
      }},
      {{
          .bind_rules = canvas_bind_rules,
          .properties = canvas_properties,
      }},
  };

  fuchsia_driver_framework::CompositeNodeSpec node_group{
      {.name = display_name, .parents2 = parents}};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('DISP');
  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, display_dev),
                                                          fidl::ToWire(fidl_arena, node_group));
  if (!result.ok()) {
    fdf::error("AddCompositeSpec Display(display_dev) request failed: {}",
               result.FormatDescription().data());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    fdf::error("AddCompositeSpec Display(display_dev) failed: {}",
               zx_status_get_string(result->error_value()));
    return result->take_error();
  }

  return zx::ok();
}

}  // namespace astro
