// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.clockimpl/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>

#include <sdk/lib/driver/component/cpp/composite_node_spec.h>
#include <sdk/lib/driver/component/cpp/node_add_args.h>
#include <soc/aml-meson/g12a-clk.h>
#include <soc/aml-s905d2/s905d2-hw.h>

#include "astro.h"

namespace astro {
namespace fpbus = fuchsia_hardware_platform_bus;
namespace fclkimpl = fuchsia_hardware_clockimpl;

static const std::vector<fpbus::Mmio> clk_mmios{
    {{
        .base = S905D2_HIU_BASE,
        .length = S905D2_HIU_LENGTH,
    }},
    {{
        .base = S905D2_DOS_BASE,
        .length = S905D2_DOS_LENGTH,
    }},
    {{
        .base = S905D2_MSR_CLK_BASE,
        .length = S905D2_MSR_CLK_LENGTH,
    }},
};

zx_status_t Astro::ClkInit() {
  fuchsia_hardware_clockimpl::wire::InitMetadata clock_init_metadata;
  clock_init_metadata.steps =
      fidl::VectorView<fuchsia_hardware_clockimpl::wire::InitStep>::FromExternal(
          clock_init_steps_.data(), clock_init_steps_.size());

  fit::result encoded_clock_init_metadata = fidl::Persist(clock_init_metadata);
  if (!encoded_clock_init_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to encode clock init metadata: %s",
           encoded_clock_init_metadata.error_value().FormatDescription().c_str());
    return encoded_clock_init_metadata.error_value().status();
  }

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  const fuchsia_hardware_clockimpl::ClockIdsMetadata kClockIdsMetadata{
      {.clock_nodes = std::vector<fuchsia_hardware_clockimpl::ClockNodeDescriptor>{

           // For CPU device.
           fclkimpl::ClockNodeDescriptor{{.clock_id = g12a_clk::CLK_SYS_PLL_DIV16}},
           fclkimpl::ClockNodeDescriptor{{.clock_id = g12a_clk::CLK_SYS_CPU_CLK_DIV16}},
           fclkimpl::ClockNodeDescriptor{{.clock_id = g12a_clk::CLK_SYS_CPU_CLK}},

           // For video decoder
           fclkimpl::ClockNodeDescriptor{{.clock_id = g12a_clk::CLK_DOS_GCLK_VDEC}},
           fclkimpl::ClockNodeDescriptor{{.clock_id = g12a_clk::CLK_DOS}},

           // For GPU
           fclkimpl::ClockNodeDescriptor{{.clock_id = g12a_clk::CLK_GP0_PLL}},
       }}};

  const fit::result encoded_clock_ids_metadata = fidl::Persist(kClockIdsMetadata);
  if (!encoded_clock_ids_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to encode clock ID's: %s",
           encoded_clock_ids_metadata.error_value().FormatDescription().c_str());
    return encoded_clock_ids_metadata.error_value().status();
  }
#endif

  const std::vector<fpbus::Metadata> clock_metadata{
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
      {{
          .id = fuchsia_hardware_clockimpl::wire::ClockIdsMetadata::kSerializableName,
          .data = encoded_clock_ids_metadata.value(),
      }},
#endif
      {{
          .id = fuchsia_hardware_clockimpl::wire::InitMetadata::kSerializableName,
          .data = std::move(encoded_clock_init_metadata.value()),
      }},
  };

  const fpbus::Node clk_dev = [&clock_metadata]() {
    fpbus::Node dev = {};
    dev.name() = "clock-controller-ff63c000";
    dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
    dev.pid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D2;
    dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_G12A_CLK;
    dev.mmio() = clk_mmios;
    dev.metadata() = clock_metadata;
    return dev;
  }();

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('CLK_');

  auto composite_spec = fuchsia_driver_framework::wire::CompositeNodeSpec::Builder(arena)
                            .name("clock-controller-ff63c000")
                            .Build();

  auto result =
      pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, clk_dev), composite_spec);
  if (!result.ok()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Clk(clk_dev) request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Clk(clk_dev) failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  clock_init_steps_.clear();

  return ZX_OK;
}

}  // namespace astro
