// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/platform-device/cpp/pdev.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf {

PDev::PDev(fidl::ClientEnd<fuchsia_hardware_platform_device::Device> client)
    : pdev_(std::move(client)) {}

zx::result<PDev> PDev::Connect(const std::shared_ptr<Namespace>& incoming,
                               std::string_view instance) {
  zx::result client =
      incoming->Connect<fuchsia_hardware_platform_device::Service::Device>(instance);
  if (client.is_error()) {
    return client.take_error();
  }
  return zx::ok(PDev(std::move(*client)));
}

fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> PDev::borrow() {
  return pdev_.client_end().borrow();
}

zx::result<fdf::MmioBuffer> PDev::MapMmio(uint32_t index, uint32_t cache_policy) const {
  zx::result pdev_mmio = GetMmio(index);
  if (pdev_mmio.is_error()) {
    return pdev_mmio.take_error();
  }
  return internal::PDevMakeMmioBufferWeak(*pdev_mmio, cache_policy);
}

zx::result<fdf::MmioBuffer> PDev::MapMmio(std::string_view name, uint32_t cache_policy) const {
  zx::result pdev_mmio = GetMmio(name);
  if (pdev_mmio.is_error()) {
    return pdev_mmio.take_error();
  }
  return internal::PDevMakeMmioBufferWeak(*pdev_mmio, cache_policy);
}

zx::result<PDev::MmioInfo> PDev::GetMmio(uint32_t index) const {
  fidl::WireResult<fuchsia_hardware_platform_device::Device::GetMmioById> result =
      pdev_->GetMmioById(index);
  if (result.status() != ZX_OK) {
    return zx::error(result.status());
  }
  if (!result->is_ok()) {
    return zx::error(result->error_value());
  }
  MmioInfo out_mmio = {};
  if (result->value()->has_offset()) {
    out_mmio.offset = result->value()->offset();
  }
  if (result->value()->has_size()) {
    out_mmio.size = result->value()->size();
  }
  if (result->value()->has_vmo()) {
    out_mmio.vmo = std::move(result->value()->vmo());
  }
  return zx::ok(std::move(out_mmio));
}

zx::result<PDev::MmioInfo> PDev::GetMmio(std::string_view name) const {
  fidl::WireResult<fuchsia_hardware_platform_device::Device::GetMmioByName> result =
      pdev_->GetMmioByName(fidl::StringView::FromExternal(name));
  if (result.status() != ZX_OK) {
    return zx::error(result.status());
  }
  if (!result->is_ok()) {
    return zx::error(result->error_value());
  }
  MmioInfo out_mmio = {};
  if (result->value()->has_offset()) {
    out_mmio.offset = result->value()->offset();
  }
  if (result->value()->has_size()) {
    out_mmio.size = result->value()->size();
  }
  if (result->value()->has_vmo()) {
    out_mmio.vmo = std::move(result->value()->vmo());
  }
  return zx::ok(std::move(out_mmio));
}

zx::result<zx::interrupt> PDev::GetInterrupt(uint32_t index, uint32_t flags) const {
  fidl::WireResult<fuchsia_hardware_platform_device::Device::GetInterruptById> result =
      pdev_->GetInterruptById(index, flags);
  if (result.status() != ZX_OK) {
    return zx::error(result.status());
  }
  if (!result->is_ok()) {
    return zx::error(result->error_value());
  }
  return zx::ok(std::move(result->value()->irq));
}

zx::result<zx::interrupt> PDev::GetInterrupt(std::string_view name, uint32_t flags) const {
  fidl::WireResult<fuchsia_hardware_platform_device::Device::GetInterruptByName> result =
      pdev_->GetInterruptByName(fidl::StringView::FromExternal(name), flags);
  if (result.status() != ZX_OK) {
    return zx::error(result.status());
  }
  if (!result->is_ok()) {
    return zx::error(result->error_value());
  }
  return zx::ok(std::move(result->value()->irq));
}

zx::result<zx::bti> PDev::GetBti(uint32_t index) const {
  fidl::WireResult<fuchsia_hardware_platform_device::Device::GetBtiById> result =
      pdev_->GetBtiById(index);
  if (result.status() != ZX_OK) {
    return zx::error(result.status());
  }
  if (!result->is_ok()) {
    return zx::error(result->error_value());
  }
  return zx::ok(std::move(result->value()->bti));
}

zx::result<zx::resource> PDev::GetSmc(uint32_t index) const {
  fidl::WireResult<fuchsia_hardware_platform_device::Device::GetSmcById> result =
      pdev_->GetSmcById(index);
  if (result.status() != ZX_OK) {
    return zx::error(result.status());
  }
  if (!result->is_ok()) {
    return zx::error(result->error_value());
  }
  return zx::ok(std::move(result->value()->smc));
}

zx::result<PDev::DeviceInfo> PDev::GetDeviceInfo() const {
  fidl::WireResult<fuchsia_hardware_platform_device::Device::GetNodeDeviceInfo> result =
      pdev_->GetNodeDeviceInfo();
  if (result.status() != ZX_OK) {
    return zx::error(result.status());
  }
  if (!result->is_ok()) {
    return zx::error(result->error_value());
  }

  DeviceInfo out_info = {};
  if (result->value()->has_vid()) {
    out_info.vid = result->value()->vid();
  }
  if (result->value()->has_pid()) {
    out_info.pid = result->value()->pid();
  }
  if (result->value()->has_did()) {
    out_info.did = result->value()->did();
  }
  if (result->value()->has_mmio_count()) {
    out_info.mmio_count = result->value()->mmio_count();
  }
  if (result->value()->has_irq_count()) {
    out_info.irq_count = result->value()->irq_count();
  }
  if (result->value()->has_bti_count()) {
    out_info.bti_count = result->value()->bti_count();
  }
  if (result->value()->has_smc_count()) {
    out_info.smc_count = result->value()->smc_count();
  }
  if (result->value()->has_metadata_count()) {
    out_info.metadata_count = result->value()->metadata_count();
  }
  if (result->value()->has_name()) {
    out_info.name = result->value()->name().get();
  }

  return zx::ok(std::move(out_info));
}

zx::result<PDev::BoardInfo> PDev::GetBoardInfo() const {
  fidl::WireResult<fuchsia_hardware_platform_device::Device::GetBoardInfo> result =
      pdev_->GetBoardInfo();
  if (result.status() != ZX_OK) {
    return zx::error(result.status());
  }
  if (!result->is_ok()) {
    return zx::error(result->error_value());
  }

  BoardInfo out_info = {};
  if (result->value()->has_vid()) {
    out_info.vid = result->value()->vid();
  }
  if (result->value()->has_pid()) {
    out_info.pid = result->value()->pid();
  }
  if (result->value()->has_board_name()) {
    out_info.board_name = result->value()->board_name().get();
  }
  if (result->value()->has_board_revision()) {
    out_info.board_revision = result->value()->board_revision();
  }
  return zx::ok(out_info);
}

zx::result<std::vector<fdf_power::PowerElementConfiguration>> PDev::GetPowerConfiguration() {
  fidl::WireResult<fuchsia_hardware_platform_device::Device::GetPowerConfiguration> result =
      pdev_->GetPowerConfiguration();
  if (!result.ok()) {
    return zx::error(result.status());
  }
  if (result->is_error()) {
    return zx::error(result->error_value());
  }

  std::vector<fdf_power::PowerElementConfiguration> configs;
  configs.reserve(result.value()->config.size());
  for (const auto& fidl_config : result.value()->config) {
    zx::result config = fdf_power::PowerElementConfiguration::FromFidl(fidl_config);
    if (config.is_error()) {
      return zx::error(config.status_value());
    }
    configs.emplace_back(std::move(config.value()));
  }
  return zx::ok(std::move(configs));
}

fit::result<fdf_power::Error, std::vector<fdf_power::ElementDesc>>
PDev::GetAndApplyPowerConfiguration(const fdf::Namespace& ns, bool use_element_runner) {
  zx::result configs = GetPowerConfiguration();
  if (configs.is_error()) {
    return fit::error(fdf_power::Error::CONFIGURATION_UNAVAILABLE);
  }

  if (configs->empty()) {
    return fit::success(std::vector<fdf_power::ElementDesc>{});
  }

  return fdf_power::ApplyPowerConfiguration(ns, configs.value(), use_element_runner);
}

namespace internal {
// Regular implementation for drivers. Tests might override this.
[[gnu::weak]] zx::result<fdf::MmioBuffer> PDevMakeMmioBufferWeak(PDev::MmioInfo& pdev_mmio,
                                                                 uint32_t cache_policy) {
  return MmioBuffer::Create(pdev_mmio.offset, pdev_mmio.size, std::move(pdev_mmio.vmo),
                            cache_policy);
}
}  // namespace internal

}  // namespace fdf

#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
