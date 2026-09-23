// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdmmc-rpmb-device.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/dispatcher.h>

#include "sdmmc-block-device.h"
#include "sdmmc-root-device.h"
#include "sdmmc-types.h"

namespace sdmmc {

fuchsia_hardware_rpmb::wire::EmmcDeviceInfo RpmbDeviceBase::GetDeviceInfo() {
  fuchsia_hardware_rpmb::wire::EmmcDeviceInfo emmc_info = {};
  memcpy(emmc_info.cid.data(), cid_.data(), cid_.size() * sizeof(cid_[0]));
  emmc_info.rpmb_size = rpmb_size_;
  emmc_info.reliable_write_sector_count = reliable_write_sector_count_;
  return emmc_info;
}

void RpmbDeviceBase::Request(fuchsia_hardware_rpmb::wire::Request request,
                             fit::callback<void(zx_status_t)> callback) {
  RpmbRequestInfo info = {
      .tx_frames = std::move(request.tx_frames),
      .callback = std::move(callback),
  };

  if (request.rx_frames) {
    info.rx_frames = {
        .vmo = std::move(request.rx_frames->vmo),
        .offset = request.rx_frames->offset,
        .size = request.rx_frames->size,
    };
  }

  sdmmc_parent_->RpmbQueue(std::move(info));
}

zx_status_t RpmbDevice::AddDevice() {
  {
    const std::string path_from_parent = std::string(sdmmc_parent()->parent()->driver_name()) +
                                         "/" + std::string(sdmmc_parent()->block_name()) + "/";
    auto result = compat_server_.Initialize(
        sdmmc_parent()->parent()->driver_incoming(), sdmmc_parent()->parent()->driver_outgoing(),
        sdmmc_parent()->parent()->driver_node_name(), kDeviceName, compat::ForwardMetadata::None(),
        std::nullopt, path_from_parent);
    if (result.is_error()) {
      return result.status_value();
    }
  }

  {
    fuchsia_hardware_rpmb::Service::InstanceHandler handler({
        .device = fit::bind_member<&RpmbDevice::Serve>(this),
    });
    auto result =
        sdmmc_parent()->parent()->driver_outgoing()->AddService<fuchsia_hardware_rpmb::Service>(
            std::move(handler));
    if (result.is_error()) {
      fdf::error("Failed to add RPMB service: {}", result.status_string());
      return result.status_value();
    }
  }

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  controller_.Bind(std::move(controller_client_end));

  fidl::Arena arena;
  std::vector<fuchsia_driver_framework::wire::Offer> offers = compat_server_.CreateOffers2(arena);
  offers.push_back(fdf::MakeOffer2<fuchsia_hardware_rpmb::Service>(arena));

  const auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                        .name(arena, kDeviceName)
                        .offers2(arena, std::move(offers))
                        .Build();

  auto result = sdmmc_parent()->block_node()->AddChild(args, std::move(controller_server_end), {});
  if (!result.ok()) {
    fdf::error("Failed to add child partition device: {}", result.status_string());
    return result.status();
  }
  return ZX_OK;
}

void RpmbDevice::Serve(fidl::ServerEnd<fuchsia_hardware_rpmb::Rpmb> request) {
  fidl::BindServer(sdmmc_parent()->parent()->driver_async_dispatcher(), std::move(request), this);
}

void RpmbDevice::GetDeviceInfo(GetDeviceInfoCompleter::Sync& completer) {
  fuchsia_hardware_rpmb::wire::EmmcDeviceInfo emmc_info = RpmbDeviceBase::GetDeviceInfo();
  auto device_info = fuchsia_hardware_rpmb::wire::DeviceInfo::WithEmmcInfo(
      fidl::ObjectView<fuchsia_hardware_rpmb::wire::EmmcDeviceInfo>::FromExternal(&emmc_info));
  completer.Reply(device_info);
}

void RpmbDevice::Request(RequestRequestView request, RequestCompleter::Sync& completer) {
  RpmbDeviceBase::Request(std::move(request->request),
                          [completer = completer.ToAsync()](zx_status_t status) mutable {
                            completer.Reply(zx::make_result(status));
                          });
}

void DriverRpmbDevice::GetDeviceInfo(fdf::Arena& arena, GetDeviceInfoCompleter::Sync& completer) {
  fuchsia_hardware_rpmb::wire::EmmcDeviceInfo emmc_info = RpmbDeviceBase::GetDeviceInfo();
  auto device_info = fuchsia_hardware_rpmb::wire::DeviceInfo::WithEmmcInfo(
      fidl::ObjectView<fuchsia_hardware_rpmb::wire::EmmcDeviceInfo>::FromExternal(&emmc_info));
  completer.buffer(arena).Reply(device_info);
}

void DriverRpmbDevice::Request(RequestRequestView request, fdf::Arena& arena,
                               RequestCompleter::Sync& completer) {
  RpmbDeviceBase::Request(
      std::move(request->request),
      [completer = completer.ToAsync(), arena = std::move(arena)](zx_status_t status) mutable {
        completer.buffer(arena).Reply(zx::make_result(status));
      });
}

}  // namespace sdmmc
