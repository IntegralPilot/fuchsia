// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.dwc2/cpp/fidl.h>
#include <fidl/fuchsia.hardware.usb.phy/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zircon-internal/align.h>
#include <stdlib.h>

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <soc/aml-common/aml-registers.h>
#include <soc/aml-s905d3/s905d3-hw.h>
#include <usb/cdc.h>
#include <usb/usb.h>

#include "nelson.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace nelson {
namespace fpbus = fuchsia_hardware_platform_bus;

static const std::vector<fpbus::Mmio> dwc2_mmios{
    {{
        .base = S905D3_USB1_BASE,
        .length = S905D3_USB1_LENGTH,
    }},
};

static const std::vector<fpbus::Irq> dwc2_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_USB1_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

static const std::vector<fpbus::Bti> dwc2_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_USB,
    }},
};

static const std::vector<fpbus::BootMetadata> usb_boot_metadata{
    {{
        // Use Bluetooth MAC address for USB ethernet as well.
        .zbi_type = ZBI_TYPE_DRV_MAC_ADDRESS,
        .zbi_extra = MACADDR_BLUETOOTH,
    }},
    {{
        // Advertise serial number over USB
        .zbi_type = ZBI_TYPE_SERIAL_NUMBER,
        .zbi_extra = 0,
    }},
};

static const std::vector<fpbus::Bti> usb_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_USB,
    }},
};

static const std::vector<fpbus::Mmio> usb_phy_mmios{
    {{
        .base = S905D3_USBCTRL_BASE,
        .length = S905D3_USBCTRL_LENGTH,
    }},
    {{
        .base = S905D3_USBPHY20_BASE,
        .length = S905D3_USBPHY20_LENGTH,
    }},
    {{
        .base = S905D3_USBPHY21_BASE,
        .length = S905D3_USBPHY21_LENGTH,
    }},
    {{
        .base = S905D3_POWER_BASE,
        .length = S905D3_POWER_LENGTH,
    }},
    {{
        .base = S905D3_SLEEP_BASE,
        .length = S905D3_SLEEP_LENGTH,
    }},
};

static const std::vector<fpbus::Irq> usb_phy_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D3_USB_IDDIG_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

zx_status_t AddUsbPhyComposite(fdf::WireSyncClient<fpbus::PlatformBus>& pbus,
                               fidl::AnyArena& fidl_arena, fdf::Arena& arena) {
  static const std::vector<fuchsia_hardware_usb_phy::UsbPhyMode> kUsbPhyModes = {
      {{.protocol = fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20,
        .dr_mode = fuchsia_hardware_usb_phy::Mode::kUnknown,
        .is_otg_capable = false}},
      {{.protocol = fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20,
        .dr_mode = fuchsia_hardware_usb_phy::Mode::kPeripheral,
        .is_otg_capable = true}},
  };

  static const fuchsia_hardware_usb_phy::Metadata kMetadata{
      {.usb_phy_modes = kUsbPhyModes, .phy_type = fuchsia_hardware_usb_phy::AmlogicPhyType::kG12A}};

  fit::result persisted_metadata = fidl::Persist(kMetadata);
  if (!persisted_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist metadata: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return persisted_metadata.error_value().status();
  }

  std::vector<fpbus::Metadata> usb_phy_metadata{
      {{
          .id = fuchsia_hardware_usb_phy::Metadata::kSerializableName,
          .data = std::move(persisted_metadata.value()),
      }},
  };

  fpbus::Node usb_phy_dev{{
      .name = "aml-usb-phy",
      .vid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC,
      .pid = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D3,
      .did = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_USB_PHY_V2,
      .mmio = usb_phy_mmios,
      .irq = usb_phy_irqs,
      .bti = usb_btis,
      .metadata = usb_phy_metadata,
  }};

  const std::vector<fuchsia_driver_framework::BindRule2> kResetRegisterRules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.registers.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::NAME,
                              bind_fuchsia_amlogic_platform::NAME_REGISTER_USB_PHY_V2_RESET),
  };

  const std::vector<fuchsia_driver_framework::NodeProperty2> kResetRegisterProperties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.registers.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, "register-reset"),
  };

  std::vector<fuchsia_driver_framework::ParentSpec2> parents{
      {kResetRegisterRules, kResetRegisterProperties}};
  auto result = pbus.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, usb_phy_dev),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "aml_usb_phy", .parents2 = parents}}));
  if (!result.ok()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Usb(usb_phy_dev) request failed: %s",
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Usb(usb_phy_dev) failed: %s",
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t AddDwc2Composite(fdf::WireSyncClient<fpbus::PlatformBus>& pbus,
                             fidl::AnyArena& fidl_arena, fdf::Arena& arena,
                             std::vector<fpbus::Metadata> usb_metadata) {
  const fpbus::Node dwc2_dev = [&]() {
    fpbus::Node dev = {};
    dev.name() = "dwc2";
    dev.vid() = bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC;
    dev.pid() = bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC;
    dev.did() = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_USB_DWC2;
    dev.mmio() = dwc2_mmios;
    dev.irq() = dwc2_irqs;
    dev.bti() = dwc2_btis;
    dev.metadata() = std::move(usb_metadata);
    dev.boot_metadata() = usb_boot_metadata;
    return dev;
  }();

  const std::vector<fuchsia_driver_framework::BindRule2> kDwc2PhyRules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.usb.phy.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                              bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC),
      fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_PID,
                              bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC),
      fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_DID,
                              bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_USB_DWC2),
  };

  const std::vector<fuchsia_driver_framework::NodeProperty2> kDwc2PhyProperties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.usb.phy.Service"),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                         bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                         bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                         bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_USB_DWC2),
  };

  const std::vector<fuchsia_driver_framework::ParentSpec2> kDwc2Parents{
      {kDwc2PhyRules, kDwc2PhyProperties}};
  auto result = pbus.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, dwc2_dev),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "dwc2_phy", .parents2 = kDwc2Parents}}));
  if (!result.ok()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Usb(dwc2_phy) request failed: %s",
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Usb(dwc2_phy) failed: %s",
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  return ZX_OK;
}

zx_status_t Nelson::UsbInit() {
  fidl::Arena<> fidl_arena;
  fdf::Arena arena('USB_');

  auto status = AddUsbPhyComposite(pbus_, fidl_arena, arena);
  if (status != ZX_OK) {
    zxlogf(ERROR, "AddUsbPhyComposite failed: %d", status);
    return status;
  }

  static const fuchsia_hardware_usb_dwc2::Metadata kDwc2Metadata({
      .dma_burst_len = fuchsia_hardware_usb_dwc2::DmaBurstLen::kIncr8,
      .usb_turnaround_time = 9,
      .rx_fifo_size = 256,   // for all OUT endpoints.
      .nptx_fifo_size = 32,  // for endpoint zero IN direction.
      .tx_fifo_sizes = {128, 128, 128},
  });

  fit::result persisted_metadata = fidl::Persist(kDwc2Metadata);
  if (persisted_metadata.is_error()) {
    zxlogf(ERROR, "Failed to persist dwc2 metadata: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return persisted_metadata.error_value().status();
  }

  std::vector<fpbus::Metadata> usb_metadata{
      {{.id = fuchsia_hardware_usb_dwc2::Metadata::kSerializableName,
        .data = std::move(persisted_metadata.value())}},
  };

  return AddDwc2Composite(pbus_, fidl_arena, arena, std::move(usb_metadata));
}

}  // namespace nelson
