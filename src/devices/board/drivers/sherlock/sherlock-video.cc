// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <zircon/syscalls/smc.h>

#include <bind/fuchsia/amlogic/platform/meson/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <soc/aml-meson/g12b-clk.h>
#include <soc/aml-t931/t931-hw.h>

#include "sherlock.h"

namespace sherlock {
namespace fpbus = fuchsia_hardware_platform_bus;

static const std::vector<fpbus::Mmio> sherlock_video_mmios{
    {{
        .base = T931_CBUS_BASE,
        .length = T931_CBUS_LENGTH,
    }},
    {{
        .base = T931_DOS_BASE,
        .length = T931_DOS_LENGTH,
    }},
    {{
        .base = T931_HIU_BASE,
        .length = T931_HIU_LENGTH,
    }},
    {{
        .base = T931_AOBUS_BASE,
        .length = 0x1000,
    }},
    {{
        .base = T931_DMC_BASE,
        .length = T931_DMC_LENGTH,
    }},
};

static const std::vector<fpbus::Bti> sherlock_video_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_VIDEO,
    }},
};

static const std::vector<fpbus::Irq> sherlock_video_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_DEMUX_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_PARSER_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_DOS_MBOX_0_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_DOS_MBOX_1_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

static const std::vector<fpbus::Smc> sherlock_video_smcs{
    {{
        .service_call_num_base = ARM_SMC_SERVICE_CALL_NUM_TRUSTED_OS_BASE,
        .count = 1,
        .exclusive = false,
    }},
};

static const fpbus::Node video_dev = []() {
  fpbus::Node dev = {};
  dev.name() = "video-decoder-ffd00000";
  dev.vid() = PDEV_VID_AMLOGIC;
  dev.pid() = PDEV_PID_AMLOGIC_T931;
  dev.did() = PDEV_DID_AMLOGIC_VIDEO;
  dev.mmio() = sherlock_video_mmios;
  dev.bti() = sherlock_video_btis;
  dev.irq() = sherlock_video_irqs;
  dev.smc() = sherlock_video_smcs;
  return dev;
}();

zx_status_t Sherlock::VideoInit() {
  fidl::Arena<> fidl_arena;
  fdf::Arena arena('VIDE');
  auto video_canvas = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE,
                                      "fuchsia.hardware.amlogiccanvas.Service"),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.amlogiccanvas.Service"),
          },
  }};

  auto video_clock_dos_vdec = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
              fdf::MakeAcceptBindRule(
                  bind_fuchsia::ID,
                  bind_fuchsia_amlogic_platform_meson::G12B_CLK_ID_CLK_DOS_GCLK_VDEC),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "clock-dos-vdec"),
          },
  }};

  auto video_clock_dos = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                                      bind_fuchsia_amlogic_platform_meson::G12B_CLK_ID_CLK_DOS),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "clock-dos"),
          },
  }};

  auto video_tee = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.tee.Service"),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.tee.Service"),
          },
  }};

  auto video_spec = fuchsia_driver_framework::CompositeNodeSpec{{
      .name = "aml_video",
      .parents2 = {{video_canvas, video_clock_dos_vdec, video_clock_dos, video_tee}},
  }};

  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, video_dev),
                                                          fidl::ToWire(fidl_arena, video_spec));
  if (!result.ok()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Video(video_dev) request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Video(video_dev) failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  return ZX_OK;
}

}  // namespace sherlock
