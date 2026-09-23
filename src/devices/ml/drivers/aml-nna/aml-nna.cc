// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-nna.h"

#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <stdlib.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <memory>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/platform/cpp/bind.h>
#include <bind/fuchsia/verisilicon/platform/cpp/bind.h>

#include "a5-nna-regs.h"
#include "s905d3-nna-regs.h"
#include "t931-nna-regs.h"

namespace aml_nna {

zx_status_t AmlNnaDriver::PowerDomainControl(bool turn_on) {
  ZX_ASSERT(smc_monitor_.is_valid());
  static const zx_smc_parameters_t kSetPdCall =
      aml_pd_smc::CreatePdSmcCall(nna_block_.nna_domain_id, turn_on ? 1 : 0);

  zx_smc_result_t result;
  zx_status_t status = zx_smc_call(smc_monitor_.get(), &kSetPdCall, &result);
  if (status != ZX_OK) {
    fdf::error("Call zx_smc_call failed: {}", zx_status_get_string(status));
  }

  return status;
}

zx::result<> AmlNnaDriver::Reset() {
  if (nna_block_.nna_power_version == kNnaPowerDomain) {
    fdf::error("Reset not yet supported for PowerDomainControl\n");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // set bit[12]=0
  auto clear_result = reset_register_->WriteRegister32(nna_block_.nna_regs.reset_level2_offset,
                                                       aml_registers::NNA_RESET2_LEVEL_MASK, 0);
  if (!clear_result.ok()) {
    fdf::error("Failed to send request to clear reset register: {}", clear_result.status_string());
    return zx::error(clear_result.status());
  }
  if (clear_result->is_error()) {
    fdf::error("Failed to clear reset register: {}",
               zx_status_get_string(clear_result->error_value()));
    return clear_result->take_error();
  }

  // set bit[12]=1
  auto set_result = reset_register_->WriteRegister32(nna_block_.nna_regs.reset_level2_offset,
                                                     aml_registers::NNA_RESET2_LEVEL_MASK,
                                                     aml_registers::NNA_RESET2_LEVEL_MASK);
  if (!set_result.ok()) {
    fdf::error("Failed to send request to set reset register: {}", set_result.status_string());
    return zx::error(set_result.status());
  }
  if (set_result->is_error()) {
    fdf::error("Failed to set reset register: {}", zx_status_get_string(set_result->error_value()));
    return set_result->take_error();
  }

  return zx::ok();
}

void AmlNnaDriver::Reset(ResetCompleter::Sync& completer) { completer.Reply(Reset()); }

zx::result<> AmlNnaDriver::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());

  {
    zx::result<> result =
        compat_server_.Initialize(incoming, outgoing(), context.node_name(), kChildNodeName,
                                  compat::ForwardMetadata::Some({0}));
    if (result.is_error()) {
      return result.take_error();
    }
  }

  zx::result reset_register_client_end =
      incoming->Connect<fuchsia_hardware_registers::Service::Device>(kResetRegisterParentName);
  if (reset_register_client_end.is_error()) {
    fdf::error("Failed to connect to reset register: {}", reset_register_client_end);
    return reset_register_client_end.take_error();
  }

  reset_register_ = fidl::WireSyncClient<fuchsia_hardware_registers::Device>(
      std::move(reset_register_client_end.value()));

  zx::result pdev_client_end = incoming->Connect<fuchsia_hardware_platform_device::Service::Device>(
      kPlatformDeviceParentName);
  if (pdev_client_end.is_error()) {
    fdf::error("Failed to connect to platform device: {}", pdev_client_end);
    return pdev_client_end.take_error();
  }
  fdf::PDev pdev{std::move(pdev_client_end.value())};

  zx::result hiu_mmio = pdev.MapMmio(kHiuMmioIndex);
  if (hiu_mmio.is_error()) {
    fdf::error("Failed to map hiu mmio: {}", hiu_mmio);
    return hiu_mmio.take_error();
  }
  hiu_mmio_.emplace(std::move(hiu_mmio.value()));

  zx::result power_mmio = pdev.MapMmio(kPowerDomainMmioIndex);
  if (power_mmio.is_error()) {
    fdf::error("Failed to map power domain mmio: {}", power_mmio);
    return power_mmio.take_error();
  }
  power_mmio_.emplace(std::move(power_mmio.value()));

  zx::result memory_pd_mmio = pdev.MapMmio(kMemoryDomainMmioIndex);
  if (memory_pd_mmio.is_error()) {
    fdf::error("Failed to map memory domain mmio: {}", memory_pd_mmio);
    return memory_pd_mmio.take_error();
  }
  memory_pd_mmio_.emplace(std::move(memory_pd_mmio.value()));

  // TODO(fxb/318736574) : Replace with GetDeviceInfo.
  zx::result info_result = pdev.GetBoardInfo();
  if (info_result.is_error()) {
    fdf::error("Failed to get board info: {}", info_result);
    return info_result.take_error();
  }
  const auto& info = info_result.value();

  uint32_t nna_pid = 0;
  if (info.vid == PDEV_VID_AMLOGIC) {
    nna_pid = info.pid;
  } else if (info.vid == PDEV_VID_GOOGLE) {
    switch (info.pid) {
      case PDEV_PID_SHERLOCK:
        nna_pid = PDEV_PID_AMLOGIC_T931;
        break;
      case PDEV_PID_NELSON:
        nna_pid = PDEV_PID_AMLOGIC_S905D3;
        break;
      default:
        fdf::error("unhandled PID {:#x} for VID {:#x}", info.pid, info.vid);
        return zx::error(ZX_ERR_INVALID_ARGS);
    }
  } else if (info.vid == PDEV_VID_KHADAS) {
    switch (info.pid) {
      case PDEV_PID_VIM3:
        nna_pid = PDEV_PID_AMLOGIC_A311D;
        break;
      default:
        fdf::error("unhandled PID {:#x} for VID {:#x}", info.pid, info.vid);
        return zx::error(ZX_ERR_INVALID_ARGS);
    }
  } else {
    fdf::error("unhandled VID {:#x}", info.vid);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  switch (nna_pid) {
    case PDEV_PID_AMLOGIC_A311D:
    case PDEV_PID_AMLOGIC_T931:
      nna_block_ = T931NnaBlock;
      break;
    case PDEV_PID_AMLOGIC_S905D3:
      nna_block_ = S905d3NnaBlock;
      break;
    case PDEV_PID_AMLOGIC_A5: {
      nna_block_ = A5NnaBlock;
      zx::result result = pdev.GetSmc(0);
      if (result.is_error()) {
        fdf::error("unable to get sip monitor handle: {}", result);
        return result.take_error();
      }
      smc_monitor_ = std::move(result.value());
      break;
    }
    default:
      fdf::error("unhandled PID {:#x}", nna_pid);
      return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const bool is_power_domain = nna_block_.nna_power_version == kNnaPowerDomain;
  fdf::info("Starting hardware: {} ", is_power_domain ? "PowerDomain" : "Registers");

  if (nna_block_.nna_power_version == kNnaPowerDomain) {
    zx_status_t status = PowerDomainControl(true);
    if (status != ZX_OK) {
      fdf::error("PowerDomainControl failed: {}\n", zx_status_get_string(status));
      return zx::error(status);
    }
  } else {
    power_mmio_->ClearBits32(nna_block_.nna_regs.domain_power_sleep_bits,
                             nna_block_.nna_regs.domain_power_sleep_offset);

    memory_pd_mmio_->Write32(0, nna_block_.nna_regs.hhi_mem_pd_reg0_offset);

    memory_pd_mmio_->Write32(0, nna_block_.nna_regs.hhi_mem_pd_reg1_offset);

    // set bit[12]=0
    auto clear_result = reset_register_->WriteRegister32(nna_block_.nna_regs.reset_level2_offset,
                                                         aml_registers::NNA_RESET2_LEVEL_MASK, 0);
    if (!clear_result.ok()) {
      fdf::error("Failed to send request to clear reset register: {}",
                 clear_result.status_string());
      return zx::error(clear_result.status());
    }
    if (clear_result->is_error()) {
      fdf::error("Failed to clear reset register: {}",
                 zx_status_get_string(clear_result->error_value()));
      return clear_result->take_error();
    }

    power_mmio_->ClearBits32(nna_block_.nna_regs.domain_power_iso_bits,
                             nna_block_.nna_regs.domain_power_iso_offset);

    // set bit[12]=1
    auto set_result = reset_register_->WriteRegister32(nna_block_.nna_regs.reset_level2_offset,
                                                       aml_registers::NNA_RESET2_LEVEL_MASK,
                                                       aml_registers::NNA_RESET2_LEVEL_MASK);
    if (!set_result.ok()) {
      fdf::error("Failed to send request to set reset register: {}", set_result.status_string());
      return zx::error(set_result.status());
    }
    if (set_result->is_error()) {
      fdf::error("Failed to set reset register: {}",
                 zx_status_get_string(set_result->error_value()));
      return set_result->take_error();
    }
  }
  // Setup Clocks.
  // VIPNANOQ Core clock
  hiu_mmio_->SetBits32(nna_block_.clock_core_control_bits, nna_block_.clock_control_offset);
  // VIPNANOQ Axi clock
  hiu_mmio_->SetBits32(nna_block_.clock_axi_control_bits, nna_block_.clock_control_offset);

  zx::result result = outgoing()->AddService<fuchsia_hardware_platform_device::Service>(
      fuchsia_hardware_platform_device::Service::InstanceHandler({
          .device =
              [incoming](fidl::ServerEnd<fuchsia_hardware_platform_device::Device> server_end) {
                zx::result result =
                    incoming->Connect<fuchsia_hardware_platform_device::Service::Device>(
                        std::move(server_end), "pdev");
                if (result.is_error()) {
                  fdf::error("Failed to connect to platform device: {}", result);
                }
              },
      }));
  if (result.is_error()) {
    fdf::error("Failed to add platform device service: {}", result);
    return result.take_error();
  }

  result = outgoing()->AddService<fuchsia_hardware_vsi::PowerParentService>(
      fuchsia_hardware_vsi::PowerParentService::InstanceHandler({
          .power =
              [this](fidl::ServerEnd<fuchsia_hardware_vsi::PowerParent> server) {
                fidl::BindServer(dispatcher(), std::move(server), this);
              },
      }));
  if (result.is_error()) {
    fdf::error("Failed to add nna service: {}", result);
    return result.take_error();
  }

  std::vector offers = compat_server_.CreateOffers2();
  offers.push_back(fdf::MakeOffer2<fuchsia_hardware_platform_device::Service>());
  offers.push_back(fdf::MakeOffer2<fuchsia_hardware_vsi::PowerParentService>());

  std::vector properties = {
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_platform::BIND_PROTOCOL_DEVICE),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                         bind_fuchsia_verisilicon_platform::BIND_PLATFORM_DEV_VID_VERISILICON),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                         bind_fuchsia_platform::BIND_PLATFORM_DEV_PID_GENERIC),
      fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                         bind_fuchsia_verisilicon_platform::BIND_PLATFORM_DEV_DID_MAGMA_VIP),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.platform.device.Service"),
  };

  zx::result child = AddChild(kChildNodeName, properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

}  // namespace aml_nna

FUCHSIA_DRIVER_EXPORT2(aml_nna::AmlNnaDriver);
