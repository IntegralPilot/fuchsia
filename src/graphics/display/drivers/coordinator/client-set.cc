// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/client-set.h"

#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/status.h>
#include <zircon/time.h>

#include <memory>
#include <span>

#include <fbl/string_printf.h>

#include "src/graphics/display/drivers/coordinator/client.h"
#include "src/graphics/display/drivers/coordinator/controller.h"
#include "src/graphics/display/lib/api-types/cpp/client-priority.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"

namespace display_coordinator {

ClientSet::ClientSet(inspect::Node root_node) : root_node_(std::move(root_node)) {}

ClientSet::~ClientSet() {
  ZX_DEBUG_ASSERT_MSG(clients_.empty(), "Clear() must be called before the ClientSet is destroyed");
}

void ClientSet::DispatchOnDisplaysChanged(std::span<const display::DisplayId> added_display_ids,
                                          std::span<const display::DisplayId> removed_display_ids) {
  for (std::unique_ptr<Client>& client : clients_) {
    client->OnDisplaysChanged(added_display_ids, removed_display_ids);
  }
}

void ClientSet::DispatchOnDisplayVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                                       display::DriverConfigStamp vsync_config_stamp,
                                       display::ClientPriority client_priority) {
  ZX_DEBUG_ASSERT(display_id != display::kInvalidDisplayId);
  ZX_DEBUG_ASSERT(vsync_config_stamp != display::kInvalidDriverConfigStamp);
  ZX_DEBUG_ASSERT(client_priority != display::ClientPriority::kInvalid);

  zx_instant_mono_t fidl_timestamp = timestamp.get();
  for (std::unique_ptr<Client>& client : clients_) {
    if (client->priority() == client_priority) {
      client->OnDisplayVsync(display_id, fidl_timestamp, vsync_config_stamp);
      break;
    }
  }
}

void ClientSet::DispatchOnCaptureComplete() {
  for (std::unique_ptr<Client>& client : clients_) {
    client->OnCaptureComplete();
  }
}

namespace {

void PrintChannelKoids(display::ClientPriority client_priority, const zx::channel& channel) {
  ZX_DEBUG_ASSERT(client_priority != display::ClientPriority::kInvalid);

  zx_info_handle_basic_t info{};
  size_t actual, avail;
  zx_status_t status = channel.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), &actual, &avail);
  if (status != ZX_OK || info.type != ZX_OBJ_TYPE_CHANNEL) {
    fdf::error("Failed to get koids for handle type {}: {}", info.type, status);
    return;
  }
  ZX_DEBUG_ASSERT(actual == avail);
  fdf::info("Client connecting at priority {} - FIDL client end: 0x{:x} server end: 0x{:x}",
            client_priority, info.related_koid, info.koid);
}

}  // namespace

zx::result<> ClientSet::ConnectClient(
    Controller* controller, display::ClientPriority client_priority,
    std::span<const display::DisplayId> current_display_ids,
    fidl::ServerEnd<fuchsia_hardware_display::Coordinator> coordinator_server_end,
    fidl::ClientEnd<fuchsia_hardware_display::CoordinatorListener>
        coordinator_listener_client_end) {
  ZX_DEBUG_ASSERT(controller != nullptr);
  ZX_DEBUG_ASSERT(client_priority != display::ClientPriority::kInvalid);
  ZX_DEBUG_ASSERT(coordinator_server_end.is_valid());
  ZX_DEBUG_ASSERT(coordinator_listener_client_end.is_valid());

  PrintChannelKoids(client_priority, coordinator_server_end.channel());

  for (const std::unique_ptr<Client>& existing_client : clients_) {
    if (existing_client->priority() == client_priority) {
      fdf::warn("Rejecting client connection. A client is already bound at priority {}",
                client_priority);
      return zx::error(ZX_ERR_ALREADY_BOUND);
    }
  }

  ClientId client_id = next_client_id_;
  ++next_client_id_;

  fbl::AllocChecker alloc_checker;
  auto client = fbl::make_unique_checked<Client>(&alloc_checker, controller, client_priority,
                                                 client_id, std::move(coordinator_server_end),
                                                 std::move(coordinator_listener_client_end));
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for Client");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  inspect::Node client_inspect_node =
      root_node_.CreateChild(fbl::StringPrintf("client-%" PRIu64, client_id.value()));
  client->AttachInspectNode(std::move(client_inspect_node));

  Client* client_ptr = client.get();
  clients_.push_back(std::move(client));

  std::span<const display::DisplayId> removed_display_ids = {};
  client_ptr->OnDisplaysChanged(current_display_ids, removed_display_ids);

  fdf::info("Client connected at priority {} with ID {}", client_priority,
            client_ptr->id().value());

  HandleClientOwnershipChanges();

  return zx::ok();
}

std::optional<display::ClientPriority> ClientSet::FindConfigStampSource(
    display::DriverConfigStamp driver_config_stamp) {
  for (const std::unique_ptr<Client>& client : clients_) {
    const std::list<Client::ConfigStampPair>& pending_stamps =
        client->pending_displayed_config_stamps();
    auto pending_stamps_it =
        std::ranges::find_if(pending_stamps, [&](const Client::ConfigStampPair& pending_stamp) {
          return pending_stamp.driver_stamp >= driver_config_stamp;
        });
    if (pending_stamps_it == pending_stamps.end()) {
      continue;
    }
    if (pending_stamps_it->driver_stamp == driver_config_stamp) {
      return std::make_optional(client->priority());
    }
  }
  return std::nullopt;
}

void ClientSet::OnClientDisconnected(Client* client) {
  ZX_DEBUG_ASSERT(client != nullptr);

  fdf::info("Client disconnected: priority {}, ID {}", client->priority(), client->id().value());

  if (client == client_owning_displays_) {
    client_owning_displays_ = nullptr;
  }

  for (auto clients_it = clients_.begin(); clients_it != clients_.end(); ++clients_it) {
    if (clients_it->get() == client) {
      clients_it->get()->ReleaseResources();
      clients_.erase(clients_it);
      break;
    }
  }

  HandleClientOwnershipChanges();
}

void ClientSet::HandleClientOwnershipChanges() {
  Client* new_client_owning_displays = nullptr;
  for (std::unique_ptr<Client>& client : clients_) {
    if (!new_client_owning_displays ||
        client->priority() > new_client_owning_displays->priority()) {
      new_client_owning_displays = client.get();
    }
  }

  if (new_client_owning_displays != client_owning_displays_) {
    if (client_owning_displays_) {
      client_owning_displays_->SetOwnership(false);
    }
    if (new_client_owning_displays) {
      new_client_owning_displays->SetOwnership(true);
    }
    client_owning_displays_ = new_client_owning_displays;
  }
}

void ClientSet::Clear() {
  for (const std::unique_ptr<Client>& client : clients_) {
    client->CloseFidlConnection(ZX_ERR_CONNECTION_ABORTED);
    client->ReleaseResources();
  }

  client_owning_displays_ = nullptr;

  clients_.clear();
}

Client* ClientSet::GetClientOwningDisplays() const { return client_owning_displays_; }

}  // namespace display_coordinator
