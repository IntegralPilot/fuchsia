// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.nand/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/io-buffer.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <unistd.h>
#include <zircon/hw/gpt.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <soc/aml-common/aml-guid.h>
#include <soc/aml-s905d2/s905d2-gpio.h>
#include <soc/aml-s905d2/s905d2-hw.h>

#include "astro.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace astro {
namespace fpbus = fuchsia_hardware_platform_bus;

static const std::vector<fpbus::Mmio> raw_nand_mmios{
    {{
        /* nandreg : Registers for NAND controller */
        .base = S905D2_RAW_NAND_REG_BASE,
        .length = 0x2000,
    }},
    {{
        /* clockreg : Clock Register for NAND controller */
        .base = S905D2_RAW_NAND_CLOCK_BASE,
        .length = 0x4,
    }},
};

static const std::vector<fpbus::Irq> raw_nand_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_RAW_NAND_IRQ),
        .mode = fpbus::ZirconInterruptMode::kDefault,
    }},
};

static const std::vector<fpbus::Bti> raw_nand_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_AML_RAW_NAND,
    }},
};

static const std::vector<fpbus::BootMetadata> raw_nand_boot_metadata{
    {{
        .zbi_type = ZBI_TYPE_DRV_PARTITION_MAP,
        .zbi_extra = 0,
    }},
};

static const std::vector<fuchsia_driver_framework::BindRule2> kGpioInitRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};
static const std::vector<fuchsia_driver_framework::NodeProperty2> kGpioInitProps = std::vector{
    fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};

static const std::vector<fuchsia_driver_framework::ParentSpec2> kRawNandParents = std::vector{
    fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProps}},
};

static const auto kCompositeNodeSpec = fuchsia_driver_framework::CompositeNodeSpec{
    {.name = "nand-ffe07800", .parents2 = kRawNandParents}};

zx_status_t Astro::RawNandInit() {
  // Set alternate functions to enable raw_nand.
  gpio_init_steps_.push_back(GpioFunction(S905D2_GPIOBOOT(8), 2));
  gpio_init_steps_.push_back(GpioFunction(S905D2_GPIOBOOT(9), 2));
  gpio_init_steps_.push_back(GpioFunction(S905D2_GPIOBOOT(10), 2));
  gpio_init_steps_.push_back(GpioFunction(S905D2_GPIOBOOT(11), 2));
  gpio_init_steps_.push_back(GpioFunction(S905D2_GPIOBOOT(12), 2));
  gpio_init_steps_.push_back(GpioFunction(S905D2_GPIOBOOT(14), 2));
  gpio_init_steps_.push_back(GpioFunction(S905D2_GPIOBOOT(15), 2));

  const fuchsia_hardware_nand::Config kNandConfig{
      {.bad_block_config{{
           .type = fuchsia_hardware_nand::BadBlockConfigType::kAmlogicUboot,
           .table_start_block = 20,
           .table_end_block = 23,
       }},
       .extra_partition_configs = {
           {{
               .type_guid = GUID_BL2_VALUE,
               .copy_count = 8,
               .copy_byte_offset = 0,
           }},
           {{
               .type_guid = GUID_BOOTLOADER_VALUE,
               .copy_count = 4,
               .copy_byte_offset = 0,
           }},
           {{
               .type_guid = GUID_SYS_CONFIG_VALUE,
               .copy_count = 4,
               .copy_byte_offset = 0,
           }},
       }}};

  fit::result persisted_nand_config = fidl::Persist(kNandConfig);
  if (!persisted_nand_config.is_ok()) {
    zxlogf(ERROR, "Failed to persist nand config: %s",
           persisted_nand_config.error_value().FormatDescription().c_str());
    return persisted_nand_config.error_value().status();
  }

  std::vector<fpbus::Metadata> metadata{
      {{
          .id = std::to_string(DEVICE_METADATA_PRIVATE),
          .data = std::move(persisted_nand_config.value()),
      }},
  };

  fpbus::Node raw_nand_dev{{
      .name = "nand-ffe07800",
      .vid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC,
      .pid = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC,
      .did = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_RAW_NAND,
      .mmio = raw_nand_mmios,
      .irq = raw_nand_irqs,
      .bti = raw_nand_btis,
      .metadata = std::move(metadata),
      .boot_metadata = raw_nand_boot_metadata,
  }};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('RAWN');
  fdf::WireUnownedResult result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, raw_nand_dev), fidl::ToWire(fidl_arena, kCompositeNodeSpec));
  if (!result.ok()) {
    zxlogf(ERROR, "%s: NodeAdd RawNand(raw_nand_dev) request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: NodeAdd RawNand(raw_nand_dev) failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

}  // namespace astro
