// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/amlogic/platform/s905d3/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <ddk/metadata/display.h>
#include <soc/aml-s905d3/s905d3-hw.h>

#include "post-init.h"
#include "src/devices/board/drivers/nelson/nelson-btis.h"

namespace nelson {
namespace fpbus = fuchsia_hardware_platform_bus;

namespace {

const std::vector<fpbus::Mmio> display_mmios{
    {{
        // VPU
        .base = S905D3_VPU_BASE,
        .length = S905D3_VPU_LENGTH,
        .name = "vpu",
    }},
    {{
        // MIPI DSI "TOP"
        .base = S905D3_MIPI_TOP_DSI_BASE,
        .length = S905D3_MIPI_TOP_DSI_LENGTH,
        .name = "dsi-top",
    }},
    {{
        // MIPI DSI PHY
        .base = S905D3_DSI_PHY_BASE,
        .length = S905D3_DSI_PHY_LENGTH,
        .name = "dsi-phy",
    }},
    {{
        // DSI Host Controller
        .base = S905D3_MIPI_DSI_BASE,
        .length = S905D3_MIPI_DSI_LENGTH,
        .name = "dsi-controller",
    }},
    {{
        // HIU / HHI
        .base = S905D3_HIU_BASE,
        .length = S905D3_HIU_LENGTH,
        .name = "hhi",
    }},
    {{
        // AOBUS
        // TODO(https://fxbug.dev/42081392): Restrict range to RTI
        .base = S905D3_AOBUS_BASE,
        .length = 0x1000,
        .name = "always-on-rti",
    }},
    {{
        // RESET
        .base = S905D3_RESET_BASE,
        .length = S905D3_RESET_LENGTH,
        .name = "ee-reset",
    }},
    {{
        // PERIPHS_REGS (GPIO Multiplexer)
        .base = S905D3_GPIO_BASE,
        .length = S905D3_GPIO_LENGTH,
        .name = "gpio-mux",
    }},
};

const std::vector<fpbus::Irq> display_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_VIU1_VSYNC_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
        .name = "viu1-vsync",
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_RDMA_DONE),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
        .name = "rdma-done",
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_VID1_WR),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
        .name = "vdin1-write-done",
    }},
};

const std::vector<fpbus::Bti> display_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_DISPLAY,
    }},
};

}  // namespace

zx::result<> PostInit::InitDisplay() {
  const std::vector<fpbus::Metadata> display_panel_metadata{
      {{
          .id = std::to_string(DEVICE_METADATA_DISPLAY_PANEL_TYPE),
          .data = std::vector<uint8_t>(
              reinterpret_cast<const uint8_t*>(&panel_type_),
              reinterpret_cast<const uint8_t*>(&panel_type_) + sizeof(display::PanelType)),
          // No metadata for this item.
      }},
  };

  fpbus::Node display_dev;
  display_dev.name() = "display";
  display_dev.vid() = PDEV_VID_AMLOGIC;
  display_dev.pid() = PDEV_PID_AMLOGIC_S905D3;
  display_dev.did() = PDEV_DID_AMLOGIC_DISPLAY;
  display_dev.metadata() = display_panel_metadata;
  display_dev.mmio() = display_mmios;
  display_dev.irq() = display_irqs;
  display_dev.bti() = display_btis;

  // Composite binding rules for display driver.
  std::vector<fuchsia_driver_framework::BindRule2> gpio_bind_rules{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                              bind_fuchsia_amlogic_platform_s905d3::GPIOZ_PIN_ID_PIN_13),
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

  fuchsia_driver_framework::CompositeNodeSpec spec{{.name = "display", .parents2 = parents}};

  // TODO(payamm): Change from "dsi" to nullptr to separate DSI and Display into two different
  // driver hosts once support for it lands.
  fidl::Arena<> fidl_arena;
  fdf::Arena arena('DISP');
  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, display_dev),
                                                          fidl::ToWire(fidl_arena, spec));
  if (!result.ok()) {
    fdf::error("AddNodeGroup Display(display_dev) request failed: {}",
               result.FormatDescription().data());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    fdf::error("AddNodeGroup Display(display_dev) failed: {}",
               zx_status_get_string(result->error_value()));
    return result->take_error();
  }

  return zx::ok();
}

}  // namespace nelson
