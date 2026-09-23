// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-virtual-bus/tests/virtual-bus-tester/host-fidl.h"

#include <fidl/fuchsia.hardware.usb/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

namespace virtualbus {

namespace fendpoint = fuchsia_hardware_usb_endpoint;

void FidlDevice::Stop(fdf::StopCompleter completer) {
  bulk_out_ep_->CancelAll().ThenExactlyOnce([](auto& result) {
    if (result.is_error()) {
      fdf::error("Failed to cancel all {}", result.error_value().FormatDescription().c_str());
    }
  });
  bulk_in_ep_->CancelAll().ThenExactlyOnce([](auto& result) {
    if (result.is_error()) {
      fdf::error("Failed to cancel all {}", result.error_value().FormatDescription().c_str());
    }
  });
  completer(zx::ok());
}

void FidlDevice::QueueOut(std::vector<uint8_t> data) {
  std::optional<usb::FidlRequest> req = bulk_out_ep_.GetRequest();
  if (!req) {
    fdf::error("No requests available");
    out_completer_->Reply(zx::error(ZX_ERR_BAD_STATE));
    out_completer_.reset();
    return;
  }

  req->clear_buffers();
  auto actual = req->CopyTo(0, data.data(), data.size(), bulk_out_ep_.GetMapped());
  (*req)->data()->at(0).size(actual[0]);
  if (actual[0] != data.size()) {
    out_completer_->Reply(zx::error(ZX_ERR_BUFFER_TOO_SMALL));
    out_completer_.reset();
    return;
  }

  std::vector<fuchsia_hardware_usb_request::Request> requests;
  requests.emplace_back(req->take_request());
  auto result = bulk_out_ep_->QueueRequests(std::move(requests));
  if (result.is_error()) {
    fdf::error("Failed to QueueRequests {}", result.error_value().FormatDescription().c_str());
    out_completer_->Reply(zx::error(ZX_ERR_INTERNAL));
    out_completer_.reset();
    return;
  }
}

void FidlDevice::QueueIn(size_t size) {
  std::optional<usb::FidlRequest> req = bulk_in_ep_.GetRequest();
  if (!req) {
    fdf::error("No requests available");
    in_completer_->Reply(zx::error(ZX_ERR_BAD_STATE));
    in_completer_.reset();
    return;
  }

  (*req)->data()->at(0).size(size);

  std::vector<fuchsia_hardware_usb_request::Request> requests;
  requests.emplace_back(req->take_request());
  auto result = bulk_in_ep_->QueueRequests(std::move(requests));
  if (result.is_error()) {
    fdf::error("Failed to QueueRequests {}", result.error_value().FormatDescription().c_str());
    in_completer_->Reply(zx::error(ZX_ERR_INTERNAL));
    in_completer_.reset();
    return;
  }
}

void FidlDevice::OutComplete(std::vector<fendpoint::Completion> completions) {
  for (auto& completion : completions) {
    bulk_out_ep_.PutRequest(usb::FidlRequest(std::move(completion.request().value())));
    if (!out_completer_) {
      continue;
    }

    *completion.status() == ZX_OK ? out_completer_->Reply(zx::ok(*completion.transfer_size()))
                                  : out_completer_->Reply(zx::error(*completion.status()));
    out_completer_.reset();
  }
}

void FidlDevice::InComplete(std::vector<fendpoint::Completion> completions) {
  for (auto& completion : completions) {
    auto req = usb::FidlRequest(std::move(completion.request().value()));
    if (!in_completer_) {
      bulk_in_ep_.PutRequest(std::move(req));
      continue;
    }

    if (*completion.status() != ZX_OK) {
      bulk_in_ep_.PutRequest(std::move(req));
      in_completer_->Reply(zx::error(*completion.status()));
      in_completer_.reset();
      continue;
    }

    auto addr = bulk_in_ep_.GetMappedAddr(req.request(), 0);
    if (!addr.has_value()) {
      zxlogf(ERROR, "Failed to get mapped");
      in_completer_->Reply(zx::error(ZX_ERR_INTERNAL));
      in_completer_.reset();
      continue;
    }

    bulk_in_ep_.PutRequest(std::move(req));
    in_completer_->Reply(zx::ok(
        std::vector<uint8_t>(reinterpret_cast<uint8_t*>(*addr),
                             reinterpret_cast<uint8_t*>(*addr) + *completion.transfer_size())));
    in_completer_.reset();
  }
}

zx::result<> FidlDevice::Start(fdf::DriverContext context) {
  config_ = context.take_config<virtual_bus_tester_config::Config>();
  zx::result result = Device::Start(std::move(context));
  if (result.is_error()) {
    fdf::error("Failed to start {}", result);
    return result.take_error();
  }

  zx::result dispatcher =
      fdf::SynchronizedDispatcher::Create({}, "ep-dispatcher", [](fdf_dispatcher_t*) {}, "");
  if (dispatcher.is_error()) {
    fdf::error("Failed to create dispatcher {}", dispatcher);
    return dispatcher.take_error();
  }
  dispatcher_ = std::move(*dispatcher);

  auto client = incoming()->Connect<fuchsia_hardware_usb::UsbService::Device>();
  if (client.is_error()) {
    fdf::error("Failed to connect fidl protocol");
    return client.take_error();
  }

  zx_status_t status = bulk_out_ep_.Init(bulk_out_addr_, *client, dispatcher_.async_dispatcher());
  if (status != ZX_OK) {
    fdf::error("Failed to init UsbEndpoint {}", zx_status_get_string(status));
    return zx::error(status);
  }

  status = bulk_in_ep_.Init(bulk_in_addr_, *client, dispatcher_.async_dispatcher());
  if (status != ZX_OK) {
    fdf::error("Failed to init UsbEndpoint {}", zx_status_get_string(status));
    return zx::error(status);
  }

  if (bulk_out_ep_.AddRequests(1, config_->vmo_data_size(),
                               fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) != 1) {
    fdf::error("Failed to register VMOs");
    return zx::error(ZX_ERR_INTERNAL);
  }

  if (bulk_in_ep_.AddRequests(1, config_->vmo_data_size(),
                              fuchsia_hardware_usb_request::Buffer::Tag::kVmoId) != 1) {
    fdf::error("Failed to register VMOs");
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok();
}

}  // namespace virtualbus

FUCHSIA_DRIVER_EXPORT2(virtualbus::FidlDevice);
