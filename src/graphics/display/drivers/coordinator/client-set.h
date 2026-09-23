// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_SET_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_SET_H_

#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/result.h>
#include <zircon/time.h>

#include <list>
#include <memory>
#include <span>

#include "src/graphics/display/drivers/coordinator/client-id.h"
#include "src/graphics/display/lib/api-types/cpp/client-priority.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"

namespace display_coordinator {

class Client;
class Controller;

// Manages all of a Display Coordinator's client connections.
//
// Instances are not thread-safe.
class ClientSet {
 public:
  // Creates an empty set.
  //
  // `root_node` will be populated with one sub-node per connected client.
  explicit ClientSet(inspect::Node root_node);

  ClientSet(const ClientSet&) = delete;
  ClientSet(ClientSet&&) = delete;
  ClientSet& operator=(const ClientSet&) = delete;
  ClientSet& operator=(ClientSet&&) = delete;

  ~ClientSet();

  // Dispatches the changes to all clients.
  void DispatchOnDisplaysChanged(std::span<const display::DisplayId> added_display_ids,
                                 std::span<const display::DisplayId> removed_display_ids);

  // Dispatches the VSync to the client that submitted the configuration.
  //
  // `display_id`, `vsync_config_stamp`, and `client_priority` must be valid.
  void DispatchOnDisplayVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                              display::DriverConfigStamp vsync_config_stamp,
                              display::ClientPriority client_priority);

  // Dispatches the event to all clients.
  void DispatchOnCaptureComplete();

  // Connects a client at the given priority level.
  //
  // After this method completes, the client will receive an OnDisplaysChanged
  // event stating that the currently connected displays are those in
  // `displays`.
  //
  // `controller` must be non-null and must outlive the ClientSet.
  // `client_priority`, `coordinator_server_end`, and
  // `coordinator_listener_client_end` must be valid.
  zx::result<> ConnectClient(
      Controller* controller, display::ClientPriority client_priority,
      std::span<const display::DisplayId> current_display_ids,
      fidl::ServerEnd<fuchsia_hardware_display::Coordinator> coordinator_server_end,
      fidl::ClientEnd<fuchsia_hardware_display::CoordinatorListener>
          coordinator_listener_client_end);

  // `client` must point to a proxy associated with a client in this set.
  //
  // This method must be called at most once for a client.
  void OnClientDisconnected(Client* client);

  // Returns the priority of the client that committed the display configuration.
  //
  // Returns nullopt if the committed configuration does not belong to any of the
  // current clients. This happens if the client that committed the configuration
  // has disconnected.
  std::optional<display::ClientPriority> FindConfigStampSource(
      display::DriverConfigStamp driver_config_stamp);

  // Removes all clients, closing their connections.
  //
  // Must be called before the ClientSet is destroyed. No client must be added
  // between the last Clear() call and the ClientSet's destruction.
  void Clear();

  // Returns null if no client owns the displays.
  Client* GetClientOwningDisplays() const;

 private:
  // Recomputes the client that owns the displays.
  //
  // Must be called at least once when the set of connected clients changes, and
  // when the virtcon mode changes.
  //
  // This method is idempotent.
  void HandleClientOwnershipChanges();

  std::list<std::unique_ptr<Client>> clients_;

  // Points into `clients_`.
  //
  // Updated by `HandleClientOwnershipChanges()`.
  Client* client_owning_displays_ = nullptr;

  ClientId next_client_id_ = ClientId(1);

  // The inspect node that lists all client connections.
  inspect::Node root_node_;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_CLIENT_SET_H_
