// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/dispatcher.h>
#include <lib/dma-buffer/phys-iter.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/defer.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <algorithm>
#include <memory>
#include <mutex>
#include <vector>

#include <usb/sdk/request-fidl.h>

#include "src/devices/usb/lib/usb-endpoint/include/usb-endpoint/sdk/usb-endpoint-server.h"

namespace usb {
namespace fendpoint = fuchsia_hardware_usb_endpoint;
namespace frequest = fuchsia_hardware_usb_request;

static const size_t kPageSize = zx_system_get_page_size();

zx::result<std::vector<dma_buffer::PhysIter>> EndpointServer::get_iter(RequestVariant& req,
                                                                       size_t max_length) const {
  std::vector<dma_buffer::PhysIter> iters;
  const auto& fidl_request = std::get<usb::FidlRequest>(req);
  if (!fidl_request->data().has_value()) {
    return zx::ok(std::move(iters));
  }
  if (sg_support_ == ScatterGatherSupport::kUnsupported && fidl_request->data()->size() > 1) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  size_t i = 0;
  std::lock_guard<std::mutex> lock(lock_);
  for (const auto& d : *fidl_request->data()) {
    if (!d.buffer().has_value() || !d.size().has_value() || *d.size() == 0) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    switch (d.buffer()->Which()) {
      case fuchsia_hardware_usb_request::Buffer::Tag::kVmoId: {
        if (!d.buffer()->vmo_id().has_value()) {
          return zx::error(ZX_ERR_INVALID_ARGS);
        }
        auto it = registered_vmos_.find(d.buffer()->vmo_id().value());
        if (it == registered_vmos_.end()) {
          return zx::error(ZX_ERR_NOT_FOUND);
        }
        const auto& registered_vmo = it->second;
        const zx_off_t vmo_offset = d.offset().value_or(0);
        const uint64_t req_size = *d.size();
        if (vmo_offset >= registered_vmo.size || req_size > registered_vmo.size - vmo_offset) {
          return zx::error(ZX_ERR_OUT_OF_RANGE);
        }
        const size_t page_idx = vmo_offset / kPageSize;
        const size_t remaining_count = registered_vmo.phys_count - page_idx;
        const zx_off_t sub_offset = vmo_offset & (kPageSize - 1);
        auto iter = dma_buffer::PhysIter{registered_vmo.phys_list + page_idx,
                                         remaining_count,
                                         kPageSize,
                                         sub_offset,
                                         req_size,
                                         max_length};
        if (sg_support_ == ScatterGatherSupport::kUnsupported && req_size > 0) {
          auto it_p = iter.begin();
          if (it_p == iter.end() || (*it_p).second < req_size) {
            return zx::error(ZX_ERR_NOT_SUPPORTED);
          }
        }
        iters.push_back(iter);
        break;
      }
      case fuchsia_hardware_usb_request::Buffer::Tag::kData: {
        auto iter = fidl_request.phys_iter(i, max_length);
        if (sg_support_ == ScatterGatherSupport::kUnsupported && *d.size() > 0) {
          auto it_p = iter.begin();
          if (it_p == iter.end() || (*it_p).second < *d.size()) {
            return zx::error(ZX_ERR_NOT_SUPPORTED);
          }
        }
        iters.push_back(iter);
        break;
      }
      default:
        fdf::error("Not supported buffer type");
        return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
    i++;
  }
  return zx::success(std::move(iters));
}

void EndpointServer::Connect(async_dispatcher_t* dispatcher,
                             fidl::ServerEnd<fendpoint::Endpoint> server_end) {
  std::lock_guard<std::mutex> lock(lock_);
  binding_ref_.emplace(fidl::BindServer(dispatcher, std::move(server_end), this,
                                        std::mem_fn(&EndpointServer::OnUnbound)));
}

namespace {
// Maximum number of completions sent in a single FIDL event to avoid exceeding channel message
// size limits and reduce serialization latency.
constexpr size_t kMaxBatchSize = fendpoint::kMaxCompletionsPerEvent;

template <typename Target>
void SendCompletionsBatch(const Target& target, std::vector<fendpoint::Completion> completions) {
  if (completions.empty()) {
    return;
  }

  // Fast path: send directly without vector chunking allocations if within limit.
  if (completions.size() <= kMaxBatchSize) {
    auto status = fidl::SendEvent(target)->OnCompletion(std::move(completions));
    if (status.is_error()) {
      fdf::error("Error sending event: {}", status.error_value().status_string());
    }
    return;
  }

  auto iter = completions.begin();
  while (iter != completions.end()) {
    size_t chunk_size =
        std::min(static_cast<size_t>(std::distance(iter, completions.end())), kMaxBatchSize);
    std::vector<fendpoint::Completion> chunk(std::make_move_iterator(iter),
                                             std::make_move_iterator(iter + chunk_size));
    iter += chunk_size;

    auto status = fidl::SendEvent(target)->OnCompletion(std::move(chunk));
    if (status.is_error()) {
      fdf::error("Error sending event: {}", status.error_value().status_string());
      break;
    }
  }
}
}  // namespace

void EndpointServer::OnUnbound(fidl::UnbindInfo info,
                               fidl::ServerEnd<fendpoint::Endpoint> server_end) {
  std::vector<fendpoint::Completion> completions;
  std::map<frequest::VmoId, RegisteredVmo> registered_vmos;
  {
    std::lock_guard<std::mutex> lock(lock_);
    completions = std::move(completions_);
    registered_vmos = std::move(registered_vmos_);
    binding_ref_.reset();
  }

  SendCompletionsBatch(server_end, std::move(completions));

  // Unregister VMOs
  auto result = UnpinVmos(registered_vmos);
  ZX_DEBUG_ASSERT(result.is_ok());

  if (info.is_user_initiated()) {
    return;
  }

  if (info.is_peer_closed()) {
    fdf::info(
        "EndpointServer ep(0x{:x}) fuchsia.hardware.usb.endpoint.Endpoint client disconnected",
        ep_addr_);
  } else {
    fdf::error("Server error: {}", info.ToError().status_string());
  }
}

void EndpointServer::RegisterVmos(RegisterVmosRequest& request,
                                  RegisterVmosCompleter::Sync& completer) {
  std::vector<fendpoint::VmoHandle> vmos;
  {
    std::lock_guard<std::mutex> lock(lock_);
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

      zx_handle_t pmt;
      size_t num_addrs = USB_ROUNDUP(size, kPageSize) / kPageSize;

      std::unique_ptr<zx_paddr_t[]> paddrs{new zx_paddr_t[num_addrs]};

      uint64_t vmo_size;
      vmo.get_size(&vmo_size);

      status = zx_bti_pin(bti_.get(), ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE, vmo.get(), 0, vmo_size,
                          paddrs.get(), num_addrs, &pmt);

      if (status != ZX_OK) {
        fdf::error("zx_bti_pin(): {}", zx_status_get_string(status));
        continue;
      }

      // Save
      vmos.emplace_back(std::move(fendpoint::VmoHandle().id(id).vmo(std::move(vmo))));
      registered_vmos_[id] = {
          .pmt = pmt, .phys_list = paddrs.release(), .phys_count = num_addrs, .size = size};
    }
  }

  completer.Reply({std::move(vmos)});
}

void EndpointServer::UnregisterVmos(UnregisterVmosRequest& request,
                                    UnregisterVmosCompleter::Sync& completer) {
  std::vector<zx_status_t> errors;
  std::vector<uint64_t> failed_vmo_ids;

  std::map<frequest::VmoId, RegisteredVmo> vmos_to_unmap;

  {
    std::lock_guard<std::mutex> lock(lock_);
    for (const auto& id : request.vmo_ids()) {
      auto registered_vmo = registered_vmos_.extract(id);
      if (registered_vmo.empty()) {
        failed_vmo_ids.emplace_back(id);
        errors.emplace_back(ZX_ERR_NOT_FOUND);
        continue;
      }
      vmos_to_unmap.emplace(id, registered_vmo.mapped());
    }
  }

  auto unpin_result = UnpinVmos(vmos_to_unmap);
  if (unpin_result.is_error()) {
    for (const auto& [id, status] : unpin_result.error_value()) {
      failed_vmo_ids.push_back(id);
      errors.push_back(status);
    }
  }
  completer.Reply({std::move(failed_vmo_ids), std::move(errors)});
}

void EndpointServer::RequestComplete(zx_status_t status, size_t actual, RequestVariant request,
                                     bool send_now) {
  auto& req = std::get<usb::FidlRequest>(request);

  auto defer_completion = *req->defer_completion();

  {
    std::lock_guard<std::mutex> lock(lock_);
    completions_.emplace_back(std::move(
        fendpoint::Completion().request(req.take_request()).status(status).transfer_size(actual)));
    if ((defer_completion && status == ZX_OK) || !send_now || !binding_ref_) {
      return;
    }
  }

  SendCompletions();
}

void EndpointServer::SendCompletions() {
  std::vector<fendpoint::Completion> completions;
  std::optional<fidl::ServerBindingRef<fendpoint::Endpoint>> binding;
  {
    std::lock_guard lock(lock_);
    if (!binding_ref_ || completions_.empty()) {
      return;
    }
    completions.swap(completions_);
    binding = *binding_ref_;
  }

  SendCompletionsBatch(*binding, std::move(completions));
}

EndpointServer::~EndpointServer() {
  std::lock_guard<std::mutex> lock(lock_);
  auto result = UnpinVmos(registered_vmos_);
  ZX_DEBUG_ASSERT(result.is_ok());
}

fit::result<std::vector<std::pair<frequest::VmoId, zx_status_t>>> EndpointServer::UnpinVmos(
    std::map<frequest::VmoId, RegisteredVmo>& vmos) {
  std::vector<std::pair<frequest::VmoId, zx_status_t>> failures;
  for (auto& [id, vmo] : vmos) {
    delete[] vmo.phys_list;
    vmo.phys_list = nullptr;
    zx_status_t status = zx_pmt_unpin(vmo.pmt);
    if (status != ZX_OK) {
      fdf::error("Failed to unpin VMO: {}", zx_status_get_string(status));
      failures.emplace_back(id, status);
    }
  }
  vmos.clear();
  if (!failures.empty()) {
    return fit::error(std::move(failures));
  }
  return fit::ok();
}

}  // namespace usb
