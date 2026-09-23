// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_MEMORY_ATTRIBUTION_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_MEMORY_ATTRIBUTION_H_

#include <fidl/fuchsia.memory.attribution/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/zx/event.h>
#include <lib/zx/process.h>
#include <zircon/types.h>

#include <optional>
#include <vector>

namespace driver_manager {

class MemoryAttributor final : public fidl::Server<fuchsia_memory_attribution::Provider> {
 public:
  explicit MemoryAttributor(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  zx::result<> Publish(component::OutgoingDirectory& outgoing);
  void AddDriver(zx::event component_token, zx_koid_t id, zx_koid_t process_koid);
  void RemoveDriver(zx_koid_t id);

  // fidl::Server<fuchsia_memory_attribution::Provider>
  void Get(GetCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_memory_attribution::Provider> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  async_dispatcher_t* dispatcher_;
  std::vector<fuchsia_memory_attribution::AttributionUpdate> pending_updates_;
  std::optional<GetCompleter::Async> pending_completer_;
  std::optional<fidl::ServerBinding<fuchsia_memory_attribution::Provider>> binding_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_MEMORY_ATTRIBUTION_H_
