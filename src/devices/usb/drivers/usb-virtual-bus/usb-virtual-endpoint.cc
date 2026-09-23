// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/usb/drivers/usb-virtual-bus/usb-virtual-endpoint.h"

#include <fbl/auto_lock.h>

#include "src/devices/usb/drivers/usb-virtual-bus/usb-virtual-bus.h"

namespace usb_virtual_bus {

namespace {

size_t GetLength(RequestVariant& req) {
  return std::holds_alternative<usb::FidlRequest>(req)
             ? std::get<usb::FidlRequest>(req).length()
             : std::get<Request>(req).request()->header.length;
}

fuchsia_hardware_usb_descriptor::UsbSetup GetSetup(RequestVariant& req) {
  if (std::holds_alternative<usb::FidlRequest>(req)) {
    return *std::get<usb::FidlRequest>(req)->information()->control()->setup();
  }

  usb_setup_t setup = std::get<Request>(req).request()->setup;
  return fuchsia_hardware_usb_descriptor::UsbSetup(setup.bm_request_type, setup.b_request,
                                                   setup.w_value, setup.w_index, setup.w_length);
}

}  // namespace

zx::result<void*> UsbEpServer::GetBuffer(RequestVariant& req) {
  if (std::holds_alternative<Request>(req)) {
    void* buffer;
    auto status = std::get<Request>(req).Mmap(&buffer);
    if (status != ZX_OK) {
      fdf::error("usb_request_mmap failed: {}", zx_status_get_string(status));
      return zx::error(status);
    }
    return zx::ok(buffer);
  }

  auto& request_buffer = (*std::get<usb::FidlRequest>(req)->data())[0].buffer();
  const auto& buffer_type = request_buffer->Which();
  switch (buffer_type) {
    case fuchsia_hardware_usb_request::Buffer::Tag::kVmoId: {
      zx::result result = GetMapped(*request_buffer);
      if (result.is_error()) {
        fdf::error("Failed to map {}", result);
        return result.take_error();
      }
      return zx::ok(reinterpret_cast<void*>(result->addr));
    }
    case fuchsia_hardware_usb_request::Buffer::Tag::kData:
      return zx::ok(request_buffer->data()->data());
    default:
      fdf::error("{}: Unknown buffer type {}", __func__, static_cast<uint32_t>(buffer_type));
      return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
}

void UsbEpServer::Connect(fidl::ServerEnd<fuchsia_hardware_usb_endpoint::Endpoint> server_end) {
  fbl::AutoLock lock(&lock_);
  if (binding_) {
    fdf::error("Endpoint already bound");
    return;
  }
  binding_ = std::make_shared<fidl::ServerBinding<fuchsia_hardware_usb_endpoint::Endpoint>>(
      ep_->bus_->async_dispatcher(), std::move(server_end), this, [this](fidl::UnbindInfo) {
        fbl::AutoLock lock(&lock_);
        for (const auto& [_, registered_vmo] : registered_vmos_) {
          auto status = zx::vmar::root_self()->unmap(registered_vmo.addr, registered_vmo.size);
          if (status != ZX_OK) {
            fdf::error("Failed to unmap VMO {}", zx_status_get_string(status));
            continue;
          }
        }
        registered_vmos_.clear();
        async::PostTask(ep_->bus_->async_dispatcher(), [this]() {
          fbl::AutoLock lock(&lock_);
          binding_.reset();
        });
      });
}

void UsbEpServer::QueueRequest(RequestVariant req) {
  if (ep_->stalled_) {
    // Do not process any requests if stalled.
    RequestComplete(ZX_ERR_IO_REFUSED, 0, req);
    return;
  }

  if (!ep_->is_control()) {
    {
      fbl::AutoLock lock(&lock_);
      pending_reqs_.push(std::move(req));
    }
    ep_->process_requests_.Post(ep_->bus_->async_dispatcher());
  } else {
    ep_->HandleControl(std::move(req));
  }
}

void UsbEpServer::CommonCancelAll() {
  std::vector<RequestVariant> reqs_to_complete;
  {
    fbl::AutoLock lock(&lock_);
    if (current_req_) {
      reqs_to_complete.push_back(std::move(current_req_->req));
      current_req_.reset();
    }
    while (!pending_reqs_.empty()) {
      reqs_to_complete.push_back(std::move(pending_reqs_.front()));
      pending_reqs_.pop();
    }
  }
  for (auto& req : reqs_to_complete) {
    RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0, req);
  }
}

void UsbEpServer::RegisterVmos(RegisterVmosRequest& request,
                               RegisterVmosCompleter::Sync& completer) {
  std::vector<fuchsia_hardware_usb_endpoint::VmoHandle> vmos;
  {
    fbl::AutoLock lock(&lock_);
    for (const auto& info : request.vmo_ids()) {
      ZX_ASSERT(info.id());
      ZX_ASSERT(info.size());
      auto id = *info.id();
      auto size = *info.size();

      if (registered_vmos_.find(id) != registered_vmos_.end()) {
        fdf::error("VMO ID {} already registered", id);
        continue;
      }

      zx::vmo vmo;
      auto status = zx::vmo::create(size, 0, &vmo);
      if (status != ZX_OK) {
        fdf::error("Failed to pin registered VMO {}", zx_status_get_string(status));
        continue;
      }

      // Map VMO.
      zx_vaddr_t mapped_addr;
      status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0, size,
                                          &mapped_addr);
      if (status != ZX_OK) {
        fdf::error("Failed to map the vmo: {}", zx_status_get_string(status));
        continue;
      }

      // Save
      vmos.emplace_back(
          std::move(fuchsia_hardware_usb_endpoint::VmoHandle().id(id).vmo(std::move(vmo))));
      registered_vmos_.emplace(id, usb::MappedVmo{mapped_addr, size});
    }
  }

  completer.Reply({std::move(vmos)});
}

void UsbEpServer::UnregisterVmos(UnregisterVmosRequest& request,
                                 UnregisterVmosCompleter::Sync& completer) {
  std::vector<zx_status_t> errors;
  std::vector<uint64_t> failed_vmo_ids;

  struct UnmapInfo {
    uint64_t id;
    usb::MappedVmo mapped;
  };
  std::vector<UnmapInfo> vmos_to_unmap;

  {
    fbl::AutoLock lock(&lock_);
    for (const auto& id : request.vmo_ids()) {
      auto node = registered_vmos_.extract(id);
      if (node.empty()) {
        failed_vmo_ids.emplace_back(id);
        errors.emplace_back(ZX_ERR_NOT_FOUND);
        continue;
      }
      vmos_to_unmap.push_back({id, std::move(node.mapped())});
    }
  }

  for (auto& info : vmos_to_unmap) {
    auto status = zx::vmar::root_self()->unmap(info.mapped.addr, info.mapped.size);
    if (status != ZX_OK) {
      fdf::error("Failed to unmap VMO {}", zx_status_get_string(status));
      failed_vmo_ids.emplace_back(info.id);
      errors.emplace_back(status);
    }
  }

  completer.Reply({std::move(failed_vmo_ids), std::move(errors)});
}

void UsbEpServer::QueueRequests(QueueRequestsRequest& request,
                                QueueRequestsCompleter::Sync& completer) {
  for (auto& req : request.req()) {
    QueueRequest(usb::FidlRequest{std::move(req)});
  }
}

void UsbEpServer::CancelAll(CancelAllCompleter::Sync& completer) {
  CommonCancelAll();
  completer.Reply(zx::ok());
}

void UsbEpServer::RequestComplete(zx_status_t status, size_t actual, RequestVariant& request) {
  if (std::holds_alternative<usb::BorrowedRequest<void>>(request)) {
    std::get<usb::BorrowedRequest<void>>(request).Complete(status, actual);
    return;
  }

  auto& freq = std::get<usb::FidlRequest>(request);
  auto defer_completion = *freq->defer_completion();

  std::vector<fuchsia_hardware_usb_endpoint::Completion> completions;

  std::shared_ptr<fidl::ServerBinding<fuchsia_hardware_usb_endpoint::Endpoint>> binding;

  {
    fbl::AutoLock lock(&lock_);

    completions_.emplace_back(std::move(fuchsia_hardware_usb_endpoint::Completion()
                                            .request(freq.take_request())
                                            .status(status)
                                            .transfer_size(actual)));
    if (defer_completion && status == ZX_OK) {
      return;
    }

    if (binding_) {
      binding = binding_;
      completions.swap(completions_);
    }
  }

  if (binding) {
    auto result = fidl::SendEvent(*binding)->OnCompletion(std::move(completions));
    if (result.is_error()) {
      fdf::error("Error sending event: {}", result.error_value().status_string());
    }
  }
}

void UsbVirtualEp::ProcessRequests() {
  ZX_DEBUG_ASSERT(!is_control());

  // Device can queue up requests when not connected to host. Host must return ZX_ERR_IO_NOT_PRESENT
  // immediately.
  if (bus_->connected_ != UsbVirtualBus::ConnectedState::kConnected &&
      bus_->connected_ != UsbVirtualBus::ConnectedState::kConnecting) {
    // Do not process any requests if not connected. Cancel all host requests.
    host_.CommonCancelAll();
    return;
  }

  struct DeferredCompletion {
    UsbEpServer* server;
    zx_status_t status;
    size_t actual;
    RequestVariant req;
  };
  std::vector<DeferredCompletion> completions;

  std::optional<UsbEpServer::CurrentRequest> host_current_req;
  std::optional<UsbEpServer::CurrentRequest> device_current_req;

  auto fill_req = [this, &completions](bool is_host,
                                       std::optional<UsbEpServer::CurrentRequest>& cur) {
    UsbEpServer& server = is_host ? host_ : device_;
    fbl::AutoLock lock(&server.lock());

    if (server.current_req_) {
      cur.emplace(std::move(*server.current_req_));
      server.current_req_.reset();
      return;
    }

    if (!server.pending_reqs_.empty()) {
      RequestVariant req = std::move(server.pending_reqs_.front());
      server.pending_reqs_.pop();

      zx::result buffer = server.GetBuffer(req);
      if (buffer.is_error()) {
        fdf::error("Failed to get buffer {}", buffer);
        completions.push_back({&server, buffer.error_value(), 0, std::move(req)});
        return;
      }

      size_t length = GetLength(req);
      cur.emplace(UsbEpServer::CurrentRequest{
          .req = std::move(req),
          .buffer = reinterpret_cast<uint8_t*>(*buffer),
          .length = length,
      });
    }
  };

  // Data transfer between device/host
  while (true) {
    if (!host_current_req) {
      fill_req(true, host_current_req);
    }
    if (!device_current_req) {
      fill_req(false, device_current_req);
    }

    if (!host_current_req || !device_current_req) {
      break;
    }

    // Transfer data (No locks held here)
    const size_t to_do = std::min(host_current_req->todo(), device_current_req->todo());
    if (is_out()) {
      std::memcpy(device_current_req->ptr(), host_current_req->ptr(), to_do);
    } else {
      std::memcpy(host_current_req->ptr(), device_current_req->ptr(), to_do);
    }

    // Complete requests
    const bool host_done = (host_current_req->offset += to_do) == host_current_req->length;
    const bool device_done = (device_current_req->offset += to_do) == device_current_req->length;

    const bool expected_data_transferred = host_done && device_done;
    // TODO(b/395946042): Support cases that need to send ZLP.
    const bool zlp = false;

    if (expected_data_transferred) {
      // Transferred exactly the requested amount
      completions.push_back(
          {&host_, ZX_OK, host_current_req->offset, std::move(host_current_req->req)});
      completions.push_back(
          {&device_, ZX_OK, device_current_req->offset, std::move(device_current_req->req)});
      host_current_req.reset();
      device_current_req.reset();
    } else if (!host_done && device_done) {
      // Non max packet size aligned transfer or ZLP. End early.
      if (is_in() && ((device_current_req->offset % max_packet_size_) != 0 || zlp)) {
        completions.push_back(
            {&host_, ZX_OK, host_current_req->offset, std::move(host_current_req->req)});
        completions.push_back(
            {&device_, ZX_OK, device_current_req->offset, std::move(device_current_req->req)});
        host_current_req.reset();
        device_current_req.reset();
      } else {
        // Host has more to transfer to device. It's ok, just transfer on the next request.
        completions.push_back(
            {&device_, ZX_OK, device_current_req->offset, std::move(device_current_req->req)});
        device_current_req.reset();
      }
    } else if (host_done && !device_done) {
      if (is_in()) {
        // IN: This is an error. Device should not send more than requested to the host.
        completions.push_back({&host_, ZX_ERR_IO_OVERRUN, 0, std::move(host_current_req->req)});
        completions.push_back({&device_, ZX_ERR_IO_OVERRUN, 0, std::move(device_current_req->req)});
        host_current_req.reset();
        device_current_req.reset();
      } else {
        // OUT: Device wants to read more from host. It's OK, just tell the device we only have so
        // much.
        completions.push_back(
            {&host_, ZX_OK, host_current_req->offset, std::move(host_current_req->req)});
        completions.push_back(
            {&device_, ZX_OK, device_current_req->offset, std::move(device_current_req->req)});
        host_current_req.reset();
        device_current_req.reset();
      }
    } else {
      ZX_ASSERT_MSG(false,
                    "Impossible state: to_do calculation error. host_done: %d, device_done: %d",
                    host_done, device_done);
    }
  }

  // Put back remaining requests if any
  if (host_current_req) {
    fbl::AutoLock lock(&host_.lock());
    host_.current_req_.emplace(std::move(*host_current_req));
  }
  if (device_current_req) {
    fbl::AutoLock lock(&device_.lock());
    device_.current_req_.emplace(std::move(*device_current_req));
  }

  // Call completions outside the lock
  for (auto& c : completions) {
    c.server->RequestComplete(c.status, c.actual, c.req);
  }
}

void UsbVirtualEp::HandleControl(RequestVariant req) {
  ZX_DEBUG_ASSERT(is_control());

  if (bus_->connected_ != UsbVirtualBus::ConnectedState::kConnected &&
      bus_->connected_ != UsbVirtualBus::ConnectedState::kConnecting) {
    // Do not process any requests if not connected.
    host_.RequestComplete(ZX_ERR_IO_NOT_PRESENT, 0, req);
    return;
  }

  fuchsia_hardware_usb_descriptor::UsbSetup setup = GetSetup(req);

  fdf::debug("{} type: 0x{:02X} req: {} value: {} index: {} length: {}", __func__,
             setup.bm_request_type(), setup.b_request(), setup.w_value(), setup.w_index(),
             setup.w_length());

  if (!bus_->dci_intf_.is_valid()) {
    fdf::error("Dci Interface not ready");
    host_.RequestComplete(ZX_ERR_UNAVAILABLE, 0, req);
    return;
  }

  const bool is_in = ((setup.bm_request_type() & USB_ENDPOINT_DIR_MASK) == USB_ENDPOINT_IN);
  std::vector<uint8_t> write(0);
  if (!is_in && setup.w_length() > 0) {
    zx::result<void*> result;
    {
      fbl::AutoLock lock(&host_.lock());
      result = host_.GetBuffer(req);
    }
    if (result.is_error()) {
      fdf::error("Failed to get buffer pointer {}", result);
      host_.RequestComplete(result.error_value(), 0, req);
      return;
    }
    write = std::vector<uint8_t>(reinterpret_cast<uint8_t*>(*result),
                                 reinterpret_cast<uint8_t*>(*result) + setup.w_length());
  }

  bus_->dci_intf_->Control({setup, std::move(write)})
      .Then([this, req = std::move(req), length = setup.w_length()](
                fidl::Result<fuchsia_hardware_usb_dci::UsbDciInterface::Control>& result) mutable {
        if (result.is_error()) {
          host_.RequestComplete(result.error_value().is_framework_error()
                                    ? ZX_ERR_IO_NOT_PRESENT
                                    : result.error_value().domain_error(),
                                0, req);
          return;
        }
        if (result->read().size() > length) {
          fdf::error("Buffer overflow!");
          host_.RequestComplete(ZX_ERR_NO_MEMORY, 0, req);
          return;
        }

        if (std::holds_alternative<usb::FidlRequest>(req) &&
            !std::get<usb::FidlRequest>(req)->data()) {
          // Means we should create a buffer for it to return.
          std::get<usb::FidlRequest>(req)->data().emplace();
          std::get<usb::FidlRequest>(req)
              ->data()
              ->emplace_back()
              .buffer(fuchsia_hardware_usb_request::Buffer::WithData(
                  std::vector<uint8_t>(result->read().size())))
              .offset(0)
              .size(result->read().size());
        }

        zx::result<void*> buffer;
        {
          fbl::AutoLock lock(&host_.lock());
          buffer = host_.GetBuffer(req);
        }
        if (buffer.is_error()) {
          fdf::error("Failed to get buffer pointer {}", buffer);
          host_.RequestComplete(buffer.error_value(), 0, req);
          return;
        }
        std::memcpy(*buffer, result->read().data(), result->read().size());
        host_.RequestComplete(ZX_OK, result->read().size(), req);
      });
}

zx::result<> UsbVirtualEp::SetStall(bool stall) {
  stalled_ = stall;

  if (stall) {
    std::vector<RequestVariant> reqs_to_complete;
    {
      fbl::AutoLock lock(&host_.lock());
      while (!host_.pending_reqs_.empty()) {
        reqs_to_complete.push_back(std::move(host_.pending_reqs_.front()));
        host_.pending_reqs_.pop();
      }
    }
    for (auto& req : reqs_to_complete) {
      host_.RequestComplete(ZX_ERR_IO_REFUSED, 0, req);
    }
  }

  return zx::ok();
}

}  // namespace usb_virtual_bus
