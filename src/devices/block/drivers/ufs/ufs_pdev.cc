// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/ufs/ufs_pdev.h"

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>

#include "src/devices/block/drivers/ufs/uic/uic_commands.h"

namespace ufs {

namespace {

constexpr uint64_t kOperationalBandwidthBps = 1'000'000'000;  // 1 GB/s
constexpr uint64_t kInactiveBandwidthBps = 0;
constexpr uint32_t kInterconnectTagUfs = 'UFS ';

}  // namespace

zx::result<> UfsPdev::InitResources() {
  auto pdev = driver_incoming()->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
  if (!pdev.is_ok()) {
    fdf::error("Failed to connect to platform device service: {}", pdev);
    return pdev.take_error();
  }

  fdf::PDev dev{std::move(pdev.value())};

  {
    auto mmio_params = dev.GetMmio(0);
    if (mmio_params.is_error()) {
      fdf::error("Failed to get MMIO: {}", mmio_params);
      return mmio_params.take_error();
    }

    mmio_buffer_vmo_ = std::move(mmio_params->vmo);
    mmio_buffer_size_ = mmio_params->size;
  }

  {
    auto bti = dev.GetBti(0);
    if (bti.is_error()) {
      fdf::error("Failed to get BTI: {}", bti);
      return bti.take_error();
    }
    bti_ = std::move(bti.value());
  }

  {
    auto irq_result = dev.GetInterrupt(0);
    if (irq_result.is_error()) {
      fdf::error("Failed to get IRQ: {}", irq_result);
      return irq_result.take_error();
    }
    irq_ = std::move(irq_result.value());
  }

  auto phy_client_end = driver_incoming()->Connect<fuchsia_hardware_ufs_phy::Service::Phy>("phy");
  if (phy_client_end.is_ok()) {
    ufs_phy_.Bind(std::move(phy_client_end.value()));
  } else {
    auto default_phy_client_end =
        driver_incoming()->Connect<fuchsia_hardware_ufs_phy::Service::Phy>();
    if (default_phy_client_end.is_ok()) {
      ufs_phy_.Bind(std::move(default_phy_client_end.value()));
    } else {
      fdf::warn("Could not connect to UFS PHY service: {}", default_phy_client_end);
    }
  }

  auto interconnect_result =
      driver_incoming()->Connect<fuchsia_hardware_interconnect::PathService::Path>(
          "ufs-interconnect");
  if (interconnect_result.is_error()) {
    fdf::error("Failed to connect to interconnect: {}", interconnect_result);
    return interconnect_result.take_error();
  }

  interconnect_client_.Bind(std::move(interconnect_result.value()));

  fidl::Arena arena;
  auto request = fuchsia_hardware_interconnect::wire::BandwidthRequest::Builder(arena)
                     .average_bandwidth_bps(kOperationalBandwidthBps)
                     .peak_bandwidth_bps(kOperationalBandwidthBps)
                     .tag(kInterconnectTagUfs)
                     .Build();
  auto result = interconnect_client_->SetBandwidth(request);
  if (!result.ok()) {
    fdf::error("SetBandwidth failed on interconnect: {}", zx_status_get_string(result.status()));
    return zx::error(result.status());
  }
  if (result->is_error()) {
    fdf::error("SetBandwidth failed on interconnect: {}",
               zx_status_get_string(result->error_value()));
    return zx::error(result->error_value());
  }

  SetHostControllerCallback(
      [this](NotifyEvent event, uint64_t data) { return PdevNotifyEventCallback(event, data); });

  return zx::ok();
}

zx_status_t UfsPdev::StopResources() {
  ZX_ASSERT(interconnect_client_.is_valid());

  fidl::Arena arena;
  auto request = fuchsia_hardware_interconnect::wire::BandwidthRequest::Builder(arena)
                     .average_bandwidth_bps(kInactiveBandwidthBps)
                     .peak_bandwidth_bps(kInactiveBandwidthBps)
                     .tag(kInterconnectTagUfs)
                     .Build();
  auto result = interconnect_client_->SetBandwidth(request);
  if (!result.ok()) {
    fdf::error("SetBandwidth failed on interconnect: {}", zx_status_get_string(result.status()));
  } else if (result->is_error()) {
    fdf::error("SetBandwidth failed on interconnect: {}",
               zx_status_get_string(result->error_value()));
  }

  StopUfshciServer();
  return ZX_OK;
}

zx::result<> UfsPdev::InitQuirk() { return zx::ok(); }

zx::result<> UfsPdev::PdevNotifyEventCallback(NotifyEvent event, uint64_t data) {
  switch (event) {
    case NotifyEvent::kPreLinkStartup:
      return PreLinkStartup();
    case NotifyEvent::kPrePowerModeChange:
      return PrePowerModeChange();
    default:
      return Ufs::NotifyEventCallback(event, data);
  }
}

// Triggers dynamic PHY calibration prior to UIC power mode / gear change
// (https://fxbug.dev/505006874).
zx::result<> UfsPdev::PrePowerModeChange() {
  if (!ufs_phy_.is_valid()) {
    return zx::ok();
  }

  auto res = ufs_phy_->PrePowerModeChange();
  if (!res.ok()) {
    fdf::error("Failed to call PrePowerModeChange: {}", res.status_string());
    return zx::error(res.status());
  }
  if (res->is_error()) {
    fdf::error("PrePowerModeChange returned error status: {}",
               zx_status_get_string(res->error_value()));
    return zx::error(res->error_value());
  }

  return zx::ok();
}

zx::result<> UfsPdev::PreLinkStartup() {
  if (!ufs_phy_.is_valid()) {
    return zx::ok();
  }

  ZX_ASSERT(!ufshci_dispatcher_.get());

  auto client_end = StartUfshciServer();
  if (client_end.is_error()) {
    return client_end.take_error();
  }

  auto res = ufs_phy_->Init(std::move(client_end.value()));
  if (!res.ok()) {
    fdf::error("Failed to call Init: {}", res.status_string());
    return zx::error(res.status());
  }
  if (res->is_error()) {
    fdf::error("Init returned error status: {}", zx_status_get_string(res->error_value()));
    return zx::error(res->error_value());
  }

  return zx::ok();
}

zx::result<fidl::ClientEnd<fuchsia_hardware_ufs_phy::Ufshci>> UfsPdev::StartUfshciServer() {
  ZX_ASSERT(!ufshci_dispatcher_.get());

  zx::result endpoints = fidl::CreateEndpoints<fuchsia_hardware_ufs_phy::Ufshci>();
  if (endpoints.is_error()) {
    fdf::error("Failed to create endpoints: {}", endpoints);
    return endpoints.take_error();
  }

  ufshci_dispatcher_shutdown_completion_.Reset();
  auto dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "ufshci-worker",
      [this](fdf_dispatcher_t*) { ufshci_dispatcher_shutdown_completion_.Signal(); });
  if (dispatcher.is_error()) {
    fdf::error("Failed to create Ufshci dispatcher: {}",
               zx_status_get_string(dispatcher.status_value()));
    return zx::error(dispatcher.status_value());
  }
  ufshci_dispatcher_ = *std::move(dispatcher);

  // Wait for bind completion, because calls to ufs_phy_ depends on the UFSHCI server working.
  libsync::Completion bind_completion;
  zx_status_t status = async::PostTask(
      ufshci_dispatcher_.async_dispatcher(),
      [this, server_end = std::move(endpoints->server), &bind_completion]() mutable {
        fidl::BindServer(ufshci_dispatcher_.async_dispatcher(), std::move(server_end), this);
        bind_completion.Signal();
      });
  if (status != ZX_OK) {
    fdf::error("Failed to post bind task: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  bind_completion.Wait();

  return zx::ok(std::move(endpoints->client));
}

void UfsPdev::StopUfshciServer() {
  if (ufshci_dispatcher_.get()) {
    ufshci_dispatcher_.ShutdownAsync();
    ufshci_dispatcher_shutdown_completion_.Wait();
    ufshci_dispatcher_ = fdf::Dispatcher();
  }
}

void UfsPdev::DmeSet(DmeSetRequest& request, DmeSetCompleter::Sync& completer) {
  DmeSetUicCommand dme_set(*this, request.mib_attribute(), request.gen_selector_index(),
                           AttrSetType::kVolatile, request.value());
  auto result = dme_set.SendCommand();
  if (result.is_error()) {
    fdf::error("DME_SET 0x{:x} failed: {}", request.mib_attribute(), result);
    completer.Reply(zx::error(result.status_value()));
  } else {
    completer.Reply(zx::ok());
  }
}

void UfsPdev::DmeGet(DmeGetRequest& request, DmeGetCompleter::Sync& completer) {
  DmeGetUicCommand dme_get(*this, request.mib_attribute(), request.gen_selector_index());
  auto result = dme_get.SendCommand();
  if (result.is_error()) {
    fdf::error("DME_GET 0x{:x} failed: {}", request.mib_attribute(), result);
    completer.Reply(zx::error(result.status_value()));
  } else {
    completer.Reply(zx::ok(*result.value()));
  }
}

}  // namespace ufs
