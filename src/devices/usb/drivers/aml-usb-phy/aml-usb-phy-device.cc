// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/aml-usb-phy/aml-usb-phy-device.h"

#include <fidl/fuchsia.hardware.registers/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/platform-device/cpp/pdev.h>

#include <mutex>

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>

#include "src/devices/usb/drivers/aml-usb-phy/aml-usb-phy.h"
#include "src/devices/usb/drivers/aml-usb-phy/power-regs.h"
#include "src/devices/usb/drivers/aml-usb-phy/usb-phy-regs.h"

namespace aml_usb_phy {
namespace {

[[maybe_unused]] void dump_power_regs(const fdf::MmioBuffer& mmio) {
  DUMP_REG(A0_RTI_GEN_PWR_SLEEP0, mmio)
  DUMP_REG(A0_RTI_GEN_PWR_ISO0, mmio)
}

[[maybe_unused]] void dump_hhi_mem_pd_regs(const fdf::MmioBuffer& mmio){
    DUMP_REG(HHI_MEM_PD_REG0, mmio)}

zx_status_t
    PowerOn(fidl::ClientEnd<fuchsia_hardware_registers::Device>& reset_register,
            fdf::MmioBuffer& power_mmio, fdf::MmioBuffer& sleep_mmio, bool dump_regs = false) {
  A0_RTI_GEN_PWR_SLEEP0::Get().ReadFrom(&sleep_mmio).set_usb_comb_power_off(0).WriteTo(&sleep_mmio);
  HHI_MEM_PD_REG0::Get().ReadFrom(&power_mmio).set_usb_comb_pd(0).WriteTo(&power_mmio);
  zx::nanosleep(zx::deadline_after(zx::usec(100)));

  fidl::Arena<> arena;
  fidl::WireUnownedResult register_result1 =
      fidl::WireCall(reset_register).buffer(arena)->WriteRegister32(RESET1_LEVEL_OFFSET, 0x4, 0);
  if (!register_result1.ok() || register_result1->is_error()) {
    fdf::error("Reset Register Write on 1 << 2 failed: {}",
               register_result1.FormatDescription().c_str());
    return ZX_ERR_INTERNAL;
  }
  zx::nanosleep(zx::deadline_after(zx::usec(100)));
  A0_RTI_GEN_PWR_ISO0::Get()
      .ReadFrom(&sleep_mmio)
      .set_usb_comb_isolation_enable(0)
      .WriteTo(&sleep_mmio);

  fidl::WireUnownedResult register_result2 =
      fidl::WireCall(reset_register).buffer(arena)->WriteRegister32(RESET1_LEVEL_OFFSET, 0x4, 0x4);
  if (!register_result2.ok() || register_result2->is_error()) {
    fdf::error("Reset Register Write on 1 << 2 failedd: {}",
               register_result2.FormatDescription().c_str());
    return ZX_ERR_INTERNAL;
  }
  zx::nanosleep(zx::deadline_after(zx::usec(100)));
  A0_RTI_GEN_PWR_SLEEP0::Get().ReadFrom(&sleep_mmio).set_pci_comb_power_off(0).WriteTo(&sleep_mmio);

  fidl::WireUnownedResult register_result3 =
      fidl::WireCall(reset_register)
          .buffer(arena)
          ->WriteRegister32(RESET1_LEVEL_OFFSET, 0xF << 26, 0);
  if (!register_result3.ok() || register_result3->is_error()) {
    fdf::error("Reset Register Write on 1 << 2 failed: {}",
               register_result3.FormatDescription().c_str());
    return ZX_ERR_INTERNAL;
  }

  A0_RTI_GEN_PWR_ISO0::Get()
      .ReadFrom(&sleep_mmio)
      .set_pci_comb_isolation_enable(0)
      .WriteTo(&sleep_mmio);
  A0_RTI_GEN_PWR_SLEEP0::Get().ReadFrom(&sleep_mmio).set_ge2d_power_off(0).WriteTo(&sleep_mmio);

  HHI_MEM_PD_REG0::Get().ReadFrom(&power_mmio).set_ge2d_pd(0).WriteTo(&power_mmio);

  A0_RTI_GEN_PWR_ISO0::Get()
      .ReadFrom(&sleep_mmio)
      .set_ge2d_isolation_enable(0)
      .WriteTo(&sleep_mmio);
  A0_RTI_GEN_PWR_ISO0::Get()
      .ReadFrom(&sleep_mmio)
      .set_ge2d_isolation_enable(1)
      .WriteTo(&sleep_mmio);

  HHI_MEM_PD_REG0::Get().ReadFrom(&power_mmio).set_ge2d_pd(0xFF).WriteTo(&power_mmio);
  A0_RTI_GEN_PWR_SLEEP0::Get().ReadFrom(&sleep_mmio).set_ge2d_power_off(1).WriteTo(&sleep_mmio);

  if (dump_regs) {
    dump_power_regs(sleep_mmio);
    dump_hhi_mem_pd_regs(power_mmio);
  }
  return ZX_OK;
}

}  // namespace

zx::result<> AmlUsbPhyDevice::Start(fdf::DriverContext context) {
  auto incoming = context.take_incoming();

  // Get Reset Register.
  fidl::ClientEnd<fuchsia_hardware_registers::Device> reset_register;
  {
    zx::result result =
        incoming->Connect<fuchsia_hardware_registers::Service::Device>("register-reset");
    if (result.is_error()) {
      fdf::error("Failed to open registers service: {}", result);
      return result.take_error();
    }
    reset_register = std::move(result.value());
  }

  zx::result pdev_client_end =
      incoming->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (pdev_client_end.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client_end);
    return pdev_client_end.take_error();
  }
  fdf::PDev pdev{std::move(pdev_client_end.value())};

  zx::result usb_phy_metadata = pdev.GetFidlMetadata<fuchsia_hardware_usb_phy::Metadata>();
  if (usb_phy_metadata.is_error()) {
    fdf::error("Failed to get metadata: {}", usb_phy_metadata);
    return usb_phy_metadata.take_error();
  }

  // Get mmio.
  std::vector<UsbPhy2> usbphy2;
  std::vector<UsbPhy3> usbphy3;
  bool needs_hack = false;
  zx::result dev_info = pdev.GetDeviceInfo();
  if (dev_info.is_ok() &&
      (dev_info->pid == bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D2 ||
       dev_info->pid == bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D3)) {
    fdf::error("Using hack");
    needs_hack = true;
  }

  zx::result usbctrl_mmio = MapMmio(pdev, 0);
  if (usbctrl_mmio.is_error()) {
    fdf::error("Failed to map mmio: {}", usbctrl_mmio);
    return usbctrl_mmio.take_error();
  }

  uint32_t idx = 1;
  const auto& usb_phy_modes = usb_phy_metadata.value().usb_phy_modes();
  if (!usb_phy_modes.has_value()) {
    fdf::error("Metadata missing usb_phy_modes field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  for (size_t i = 0; i < usb_phy_modes.value().size(); ++i) {
    zx::result mmio = MapMmio(pdev, idx);
    if (mmio.is_error()) {
      return mmio.take_error();
    }
    const auto& phy_mode = usb_phy_modes.value()[i];
    const auto& protocol = phy_mode.protocol();
    if (!protocol.has_value()) {
      fdf::error("Phy-mode {} missing protocol field", i);
      return zx::error(ZX_ERR_INTERNAL);
    }
    const auto& is_otg_capable = phy_mode.is_otg_capable();
    if (!is_otg_capable.has_value()) {
      fdf::error("Phy-mode {} missing is_otg_capable field", i);
      return zx::error(ZX_ERR_INTERNAL);
    }
    const auto& dr_mode = phy_mode.dr_mode();
    if (!is_otg_capable.has_value()) {
      fdf::error("Phy-mode {} missing dr_mode field", i);
      return zx::error(ZX_ERR_INTERNAL);
    }

    switch (protocol.value()) {
      case fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20: {
        usbphy2.emplace_back(usbphy2.size(), std::move(*mmio), is_otg_capable.value(),
                             dr_mode.value());
      } break;
      case fuchsia_hardware_usb_phy::ProtocolVersion::kUsb30: {
        usbphy3.emplace_back(usbphy3.size(), std::move(*mmio), is_otg_capable.value(),
                             dr_mode.value());
      } break;
      default:
        fdf::error("Unsupported protocol type {}", static_cast<uint32_t>(protocol.value()));
        break;
    }
    idx++;
  }

  zx::result interrupt = pdev.GetInterrupt(0);
  if (interrupt.is_error()) {
    fdf::error("Failed to get interrupt: {}", interrupt);
    return interrupt.take_error();
  }

  // Optional MMIOs
  {
    auto power_mmio = MapMmio(pdev, idx++);
    auto sleep_mmio = MapMmio(pdev, idx++);
    if (power_mmio.is_ok() && sleep_mmio.is_ok()) {
      fdf::info("Found power and sleep MMIO.");
      auto status = PowerOn(reset_register, *power_mmio, *sleep_mmio);
      if (status != ZX_OK) {
        fdf::error("Failed to power on: {}", zx_status_get_string(status));
        return zx::error(status);
      }
    }
  }

  // Create and initialize device
  const auto& phy_type = usb_phy_metadata.value().phy_type();
  if (!phy_type.has_value()) {
    fdf::error("Metadata missing phy_type field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  device_ = std::make_unique<AmlUsbPhy>(this, phy_type.value(), std::move(reset_register),
                                        std::move(*usbctrl_mmio), std::move(interrupt.value()),
                                        std::move(usbphy2), std::move(usbphy3), needs_hack);

  {
    auto result = CreateNode();
    if (result.is_error()) {
      fdf::error("Failed to create node: {}", result);
      return zx::error(result.status_value());
    }
  }

  // Initialize device. Must come after CreateNode() because Init() will create xHCI and DWC2
  // nodes on top of node_.
  auto status = device_->Init();
  if (status != ZX_OK) {
    fdf::error("Init() error {}", zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok();
}

zx::result<> AmlUsbPhyDevice::CreateNode() {
  // Add node for aml-usb-phy.
  fidl::Arena arena;
  auto args =
      fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena).name(arena, kDeviceName).Build();

  auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  zx::result node_endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::Node>();
  if (node_endpoints.is_error()) {
    fdf::error("Failed to create node endpoints: {}", node_endpoints);
    return node_endpoints.take_error();
  }

  {
    fidl::WireResult result = fidl::WireCall(node())->AddChild(
        args, std::move(controller_endpoints.server), std::move(node_endpoints->server));
    if (!result.ok()) {
      fdf::error("Failed to add child {}", result.FormatDescription().c_str());
      return zx::error(result.status());
    }
  }
  controller_.Bind(std::move(controller_endpoints.client), dispatcher());
  node_client_ = std::move(node_endpoints->client);

  return zx::ok();
}

zx::result<> AmlUsbPhyDevice::ChildNode::Publish() {
  std::lock_guard<std::mutex> _(lock_);
  count_++;
  if (count_ != 1) {
    return zx::ok();
  }

  // Serve fuchsia_hardware_usb_phy.
  {
    auto result = parent_->outgoing()->AddService<fuchsia_hardware_usb_phy::Service>(
        fuchsia_hardware_usb_phy::Service::InstanceHandler({
            .device = parent_->bindings_.CreateHandler(
                parent_->device_.get(), fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                fidl::kIgnoreBindingClosure),
        }),
        name_);
    if (result.is_error()) {
      fdf::error("Failed to add Device service: {} for device {}", result, name_);
      return result.take_error();
    }
  }

  fuchsia_driver_framework::Offer offers[]{
      fdf::MakeOffer2<fuchsia_hardware_usb_phy::Service>(name_)};
  auto properties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::NAME, name_),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                         bind_fuchsia_platform::BIND_PLATFORM_DEV_VID_GENERIC),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                         bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID, property_did_),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.usb.phy.Service"),
  };
  zx::result child = fdf::AddChild(parent_->node_client_.borrow(), *fdf::Logger::GlobalInstance(),
                                   name_, properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {} for device {}", child, name_);
    return child.take_error();
  }
  child_controller_.Bind(std::move(child.value()), parent_->dispatcher(), this);
  fdf::info("Add {} completed", name_.data());
  return zx::ok();
}

void AmlUsbPhyDevice::ChildNode::UnPublish(UnPublishCompleter completer) {
  std::lock_guard<std::mutex> _(lock_);
  if (count_ == 0) {
    // Nothing to remove.
    completer(zx::ok());
    return;
  }
  count_--;
  if (count_ != 0) {
    // Has more instances.
    completer(zx::ok());
    return;
  }

  // Reset.
  if (child_controller_.is_valid()) {
    completer_ = std::move(completer);
    auto result = child_controller_->Remove();
    if (!result.ok()) {
      fdf::error("Failed to remove {}. Status: {}", name_.data(), result.status_string());
      child_controller_ = {};
      UnPublishCompleter c = std::move(completer_);
      if (c) {
        c(zx::error(result.status()));
      }
    }
  } else {
    completer(zx::ok());
  }

  {
    zx::result result =
        parent_->outgoing()->RemoveService<fuchsia_hardware_usb_phy::Service>(name_);
    if (result.is_error()) {
      fdf::error("Failed to remove device service for {}:{}", name_.data(), result);
    }
  }
}

void AmlUsbPhyDevice::ChildNode::on_fidl_error(fidl::UnbindInfo info) {
  UnPublishCompleter completer;
  {
    std::lock_guard<std::mutex> _(lock_);
    completer = std::move(completer_);
    child_controller_ = {};
  }
  if (completer) {
    if (info.is_peer_closed()) {
      completer(zx::ok());
    } else {
      completer(zx::error(info.status()));
    }
  }
  fdf::info("Remove {} completed", name_.data());
}

void AmlUsbPhyDevice::Stop(fdf::StopCompleter completer) {
  if (controller_.is_valid()) {
    (void)controller_->Remove();
  }
  completer(zx::ok());
}

zx::result<fdf::MmioBuffer> AmlUsbPhyDevice::MapMmio(fdf::PDev& pdev, uint32_t idx) {
  return pdev.MapMmio(idx);
}

void AmlUsbPhyDevice::UnbindOnFailure() {
  if (controller_.is_valid()) {
    (void)controller_->Remove();
  }
}

}  // namespace aml_usb_phy

FUCHSIA_DRIVER_EXPORT2(aml_usb_phy::AmlUsbPhyDevice);
