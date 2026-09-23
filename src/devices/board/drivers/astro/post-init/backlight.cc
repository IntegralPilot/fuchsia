// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.ti.metadata/cpp/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/compiler.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/ti/platform/cpp/bind.h>
#include <soc/aml-s905d2/s905d2-hw.h>

#include "src/devices/board/drivers/astro/post-init/post-init.h"

namespace astro {
namespace fpbus = fuchsia_hardware_platform_bus;

static const std::vector<fpbus::Mmio> backlight_mmios{
    {{
        .base = S905D2_GPIO_AO_BASE,
        .length = S905D2_GPIO_AO_LENGTH,
    }},
};

zx::result<> PostInit::InitBacklight() {
  static const fuchsia_hardware_ti_metadata::Lp8556Metadata kMetadata(
      {.panel_id = 0,
       .allow_set_current_scale = false,
       .registers =
           std::vector<fuchsia_hardware_ti_metadata::Register>{
               // Device Control
               // EPROM
               {{.address = 0x01, .value = 0x85}},

               // CFG2
               {{.address = 0xa2, .value = 0x30}},

               // CFG3
               {{.address = 0xa3, .value = 0x32}},

               // CFG5
               {{.address = 0xa5, .value = 0x54}},

               // CFG7
               {{.address = 0xa7, .value = 0xf4}},

               // CFG9
               {{.address = 0xa9, .value = 0x60}},

               // CFGE
               {{.address = 0xae, .value = 0x09}},
           },
       .backlight_max_brightness = 400.0});

  fit::result persisted_metadata = fidl::Persist(kMetadata);
  if (!persisted_metadata.is_ok()) {
    fdf::error("Failed to persist metadata: {}",
               persisted_metadata.error_value().FormatDescription().c_str());
    return zx::error(persisted_metadata.error_value().status());
  }

  const std::vector<fpbus::Metadata> backlight_metadata{
      {{
          .id = fuchsia_hardware_ti_metadata::Lp8556Metadata::kSerializableName,
          .data = std::move(persisted_metadata.value()),
      }},
      {{
          .id = std::to_string(DEVICE_METADATA_DISPLAY_PANEL_TYPE),
          .data = std::vector<uint8_t>(
              reinterpret_cast<const uint8_t*>(&panel_type_),
              reinterpret_cast<const uint8_t*>(&panel_type_) + sizeof(panel_type_)),
      }},
  };

  std::string backlight_name;
  if (panel_type_ == display::PanelType::kBoeTv070wsmFitipowerJd9364Astro) {
    backlight_name = "backlight-boe-2c";
  } else if (panel_type_ == display::PanelType::kInnoluxP070acbFitipowerJd9364) {
    backlight_name = "backlight-innolux-2c";
  } else {
    fdf::error("Unknown panel type: {}", static_cast<uint32_t>(panel_type_));
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fpbus::Node backlight_dev = {};
  backlight_dev.name() = backlight_name;
  backlight_dev.vid() = bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_VID_TI;
  backlight_dev.pid() = bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_PID_LP8556;
  backlight_dev.did() = bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_DID_BACKLIGHT;
  backlight_dev.metadata() = backlight_metadata;
  backlight_dev.mmio() = backlight_mmios;

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('BACK');

  const auto bind_rules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::NAME, "backlight"),
  };

  const auto properties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
  };

  const auto parents = std::vector{
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = bind_rules,
          .properties = properties,
      }},
  };

  const auto composite_node_spec =
      fuchsia_driver_framework::CompositeNodeSpec{{.name = backlight_name, .parents2 = parents}};

  auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, backlight_dev), fidl::ToWire(fidl_arena, composite_node_spec));

  if (!result.ok()) {
    fdf::error("AddCompositeNodeSpec Backlight(backlight_dev) request failed: {}",
               result.FormatDescription().data());
    return zx::error(result.status());
  }
  if (result->is_error()) {
    fdf::error("AddCompositeNodeSpec Backlight(backlight_dev) failed: {}",
               zx_status_get_string(result->error_value()));
    return result->take_error();
  }

  return zx::ok();
}

}  // namespace astro
