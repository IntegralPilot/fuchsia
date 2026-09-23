// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/amlogic/platform/t931/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <ddk/metadata/display.h>
#include <soc/aml-t931/t931-gpio.h>
#include <soc/aml-t931/t931-hw.h>

#include "src/devices/board/drivers/sherlock/post-init/post-init.h"
#include "src/devices/board/drivers/sherlock/sherlock-btis.h"

namespace sherlock {
namespace fpbus = fuchsia_hardware_platform_bus;

namespace {
static const std::vector<fpbus::Mmio> display_mmios{
    {{
        // VPU
        .base = T931_VPU_BASE,
        .length = T931_VPU_LENGTH,
        .name = "vpu",
    }},
    {{
        // MIPI DSI "TOP"
        .base = T931_TOP_MIPI_DSI_BASE,
        .length = T931_TOP_MIPI_DSI_LENGTH,
        .name = "dsi-top",
    }},
    {{
        // MIPI DSI PHY
        .base = T931_DSI_PHY_BASE,
        .length = T931_DSI_PHY_LENGTH,
        .name = "dsi-phy",
    }},
    {{
        // DSI Host Controller
        .base = T931_MIPI_DSI_BASE,
        .length = T931_MIPI_DSI_LENGTH,
        .name = "dsi-controller",
    }},
    {{
        // HIU / HHI
        .base = T931_HIU_BASE,
        .length = T931_HIU_LENGTH,
        .name = "hhi",
    }},
    {{
        // AOBUS
        // TODO(https://fxbug.dev/42081392): Restrict range to RTI
        .base = T931_AOBUS_BASE,
        .length = 0x1000,
        .name = "always-on-rti",
    }},
    {{
        // RESET
        .base = T931_RESET_BASE,
        .length = T931_RESET_LENGTH,
        .name = "ee-reset",
    }},
    // gpio-mux (T931_GPIO_BASE) is omitted as it is only needed
    // for HDMI host configuration and not required for DSI panels.
};

static const std::vector<fpbus::Irq> display_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_VIU1_VSYNC_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
        .name = "viu1-vsync",
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_RDMA_DONE),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
        .name = "rdma-done",
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_VID1_WR),
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

}  // namespace

zx::result<> PostInit::InitDisplay() {
  std::vector<fpbus::Metadata> display_panel_metadata{
      {{
          .id = std::to_string(DEVICE_METADATA_DISPLAY_PANEL_TYPE),
          .data = std::vector<uint8_t>(
              reinterpret_cast<const uint8_t*>(&panel_type_),
              reinterpret_cast<const uint8_t*>(&panel_type_) + sizeof(display::PanelType)),
      }},
  };

  static const fpbus::Node display_dev = [&]() {
    fpbus::Node dev = {};
    dev.name() = "display-ff900000";
    dev.vid() = PDEV_VID_AMLOGIC;
    dev.pid() = PDEV_PID_AMLOGIC_T931;
    dev.did() = PDEV_DID_AMLOGIC_DISPLAY;
    dev.metadata() = std::move(display_panel_metadata);
    dev.mmio() = display_mmios;
    dev.irq() = display_irqs;
    dev.bti() = display_btis;
    return dev;
  }();

  std::vector<fuchsia_driver_framework::BindRule2> gpio_bind_rules{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_t931::GPIOH_PIN_ID_PIN_6),
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

  auto spec = fuchsia_driver_framework::CompositeNodeSpec{
      {.name = "display-ff900000", .parents2 = parents}};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('DISP');
  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, display_dev),
                                                          fidl::ToWire(fidl_arena, spec));
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

}  // namespace sherlock
