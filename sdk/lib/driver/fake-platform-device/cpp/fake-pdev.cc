// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/fake-bti/cpp/fake-bti.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/fake-resource/cpp/fake-resource.h>
#include <lib/driver/platform-device/cpp/pdev.h>

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)

namespace fdf_fake {

zx::result<fuchsia_hardware_platform_device::wire::Mmio> FakePDev::GetMmioById(
    uint32_t index, fidl::AnyArena& arena) {
  auto mmio = config_.mmios.find(index);
  if (mmio == config_.mmios.end()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  auto builder = fuchsia_hardware_platform_device::wire::Mmio::Builder(arena);
  if (auto* mmio_info = std::get_if<fdf::PDev::MmioInfo>(&mmio->second); mmio_info) {
    builder.offset(mmio_info->offset).size(mmio_info->size);
    zx::vmo dup;
    mmio_info->vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup);
    builder.vmo(std::move(dup));
  } else {
    // If the user has provided an `fdf::MmioBuffer` directly, we provide its
    // raw address in `offset`; the driver may access the `fdf::MmioBuffer`
    // by using the `fdf::PDev::GetMmio()` helper or explicitly calling
    // `fdf::internal::PDevMakeMmioBufferWeak()`.
    auto& mmio_buffer = std::get<fdf::MmioBuffer>(mmio->second);
    builder.offset(reinterpret_cast<size_t>(&mmio_buffer));
  }
  return zx::ok(builder.Build());
}

void FakePDev::GetMmioById(GetMmioByIdRequestView request, GetMmioByIdCompleter::Sync& completer) {
  fidl::Arena arena;
  zx::result mmio = GetMmioById(request->index, arena);
  if (mmio.is_error()) {
    completer.ReplyError(mmio.status_value());
    return;
  }
  completer.ReplySuccess(std::move(mmio.value()));
}

void FakePDev::GetMmioByName(GetMmioByNameRequestView request,
                             GetMmioByNameCompleter::Sync& completer) {
  auto index = config_.mmio_names.find(request->name.get());
  if (index == config_.mmio_names.end()) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }
  fidl::Arena arena;
  zx::result mmio = GetMmioById(index->second, arena);
  if (mmio.is_error()) {
    completer.ReplyError(mmio.status_value());
    return;
  }
  completer.ReplySuccess(std::move(mmio.value()));
}

zx::result<zx::interrupt> FakePDev::GetInterruptById(uint32_t index) {
  auto itr = config_.irqs.find(index);
  if (itr == config_.irqs.end()) {
    if (!config_.use_fake_irq) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }
    zx::interrupt irq;
    if (zx_status_t status = zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &irq);
        status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(std::move(irq));
  }
  zx::interrupt irq;
  itr->second.duplicate(ZX_RIGHT_SAME_RIGHTS, &irq);
  return zx::ok(std::move(irq));
}

zx::result<zx::bti> FakePDev::GetBtiById(uint32_t index) {
  auto itr = config_.btis.find(index);
  if (itr == config_.btis.end()) {
    if (!config_.use_fake_bti) {
      return zx::error(ZX_ERR_NOT_FOUND);
    }

    return fake_bti::CreateFakeBti();
  }
  zx::bti bti;
  itr->second.duplicate(ZX_RIGHT_SAME_RIGHTS, &bti);
  return zx::ok(std::move(bti));
}

void FakePDev::GetInterruptById(GetInterruptByIdRequestView request,
                                GetInterruptByIdCompleter::Sync& completer) {
  zx::result irq = GetInterruptById(request->index);
  if (irq.is_error()) {
    completer.ReplyError(irq.status_value());
    return;
  }
  completer.ReplySuccess(std::move(irq.value()));
}

void FakePDev::GetInterruptByName(GetInterruptByNameRequestView request,
                                  GetInterruptByNameCompleter::Sync& completer) {
  auto index = config_.irq_names.find(request->name.get());
  if (index == config_.irq_names.end()) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }
  zx::result irq = GetInterruptById(index->second);
  if (irq.is_error()) {
    completer.ReplyError(irq.status_value());
    return;
  }
  completer.ReplySuccess(std::move(irq.value()));
}

void FakePDev::GetBtiById(GetBtiByIdRequestView request, GetBtiByIdCompleter::Sync& completer) {
  zx::result bti = GetBtiById(request->index);
  if (bti.is_error()) {
    completer.ReplyError(bti.status_value());
    return;
  }
  completer.ReplySuccess(std::move(bti.value()));
}

void FakePDev::GetBtiByName(GetBtiByNameRequestView request,
                            GetBtiByNameCompleter::Sync& completer) {
  auto index = config_.bti_names.find(request->name.get());
  if (index == config_.bti_names.end()) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    return;
  }
  zx::result bti = GetBtiById(index->second);
  if (bti.is_error()) {
    completer.ReplyError(bti.status_value());
    return;
  }
  completer.ReplySuccess(std::move(bti.value()));
}

void FakePDev::GetSmcById(GetSmcByIdRequestView request, GetSmcByIdCompleter::Sync& completer) {
  zx::resource smc;
  auto itr = config_.smcs.find(request->index);
  if (itr == config_.smcs.end()) {
    if (!config_.use_fake_smc) {
      completer.ReplyError(ZX_ERR_NOT_FOUND);
      return;
    }
    zx_status_t status = fake_resource_create(ZX_RSRC_KIND_SMC, smc.reset_and_get_address());
    if (status != ZX_OK) {
      return;
    }
  } else {
    itr->second.duplicate(ZX_RIGHT_SAME_RIGHTS, &smc);
  }
  completer.ReplySuccess(std::move(smc));
}

void FakePDev::GetSmcByName(GetSmcByNameRequestView request,
                            GetSmcByNameCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void FakePDev::GetNodeDeviceInfo(GetNodeDeviceInfoCompleter::Sync& completer) {
  if (!config_.device_info.has_value()) {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }
  fdf::PDev::DeviceInfo& info = config_.device_info.value();
  fidl::Arena arena;
  completer.ReplySuccess(fuchsia_hardware_platform_device::wire::NodeDeviceInfo::Builder(arena)
                             .vid(info.vid)
                             .pid(info.pid)
                             .did(info.did)
                             .mmio_count(info.mmio_count)
                             .irq_count(info.irq_count)
                             .bti_count(info.bti_count)
                             .smc_count(info.smc_count)
                             .metadata_count(info.metadata_count)
                             .name(info.name)
                             .Build());
}

void FakePDev::GetBoardInfo(GetBoardInfoCompleter::Sync& completer) {
  if (!config_.board_info.has_value()) {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }
  fdf::PDev::BoardInfo& info = config_.board_info.value();
  fidl::Arena arena;
  completer.ReplySuccess(fuchsia_hardware_platform_device::wire::BoardInfo::Builder(arena)
                             .vid(info.vid)
                             .pid(info.pid)
                             .board_name(info.board_name)
                             .board_revision(info.board_revision)
                             .Build());
}

void FakePDev::GetPowerConfiguration(GetPowerConfigurationCompleter::Sync& completer) {
  static fidl::Arena arena;
  fidl::VectorView<fuchsia_hardware_power::wire::PowerElementConfiguration> value;
  value.Allocate(arena, config_.power_elements.size());
  for (size_t i = 0; i < config_.power_elements.size(); i++) {
    value[i] = fidl::ToWire(arena, config_.power_elements[i]);
  }
  completer.ReplySuccess(value);
}

void FakePDev::GetMetadata(GetMetadataRequestView request, GetMetadataCompleter::Sync& completer) {
  if (auto metadata = metadata_.find(request->id.get()); metadata != metadata_.end()) {
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(metadata->second));
    return;
  }

  completer.ReplyError(ZX_ERR_NOT_FOUND);
}

void FakePDev::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_platform_device::Device> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {}

}  // namespace fdf_fake

zx::result<fdf::MmioBuffer> fdf::internal::PDevMakeMmioBufferWeak(fdf::PDev::MmioInfo& pdev_mmio,
                                                                  uint32_t cache_policy) {
  if (pdev_mmio.vmo.is_valid()) {
    return MmioBuffer::Create(pdev_mmio.offset, pdev_mmio.size, std::move(pdev_mmio.vmo),
                              cache_policy);
  }

  auto* mmio_buffer = reinterpret_cast<MmioBuffer*>(pdev_mmio.offset);
  return zx::ok(std::move(*mmio_buffer));
}

#endif  // FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
