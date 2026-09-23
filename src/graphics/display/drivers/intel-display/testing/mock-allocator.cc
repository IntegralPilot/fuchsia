// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/testing/mock-allocator.h"

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <zircon/assert.h>

#include <memory>
#include <string>
#include <unordered_map>
#include <vector>

#include "src/graphics/display/drivers/intel-display/testing/fake-buffer-collection.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"

namespace intel_display {

MockAllocator::MockAllocator(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {
  ZX_ASSERT(dispatcher_);
}

MockAllocator::~MockAllocator() = default;

void MockAllocator::BindSharedCollection(BindSharedCollectionRequestView request,
                                         BindSharedCollectionCompleter::Sync& completer) {
  display::DriverBufferCollectionId buffer_collection_id = next_buffer_collection_id_++;
  active_buffer_collections_[buffer_collection_id] = {
      .token_client = std::move(request->token()),
      .mock_buffer_collection =
          std::make_unique<FakeBufferCollection>(new_buffer_collection_config_),
  };
  most_recent_buffer_collection_ =
      active_buffer_collections_.at(buffer_collection_id).mock_buffer_collection.get();

  fidl::BindServer(
      dispatcher_, std::move(request->buffer_collection_request()),
      active_buffer_collections_[buffer_collection_id].mock_buffer_collection.get(),
      [this, buffer_collection_id](FakeBufferCollection*, fidl::UnbindInfo,
                                   fidl::ServerEnd<fuchsia_sysmem2::BufferCollection>) {
        inactive_buffer_collection_tokens_.push_back(
            std::move(active_buffer_collections_[buffer_collection_id].token_client));
        active_buffer_collections_.erase(buffer_collection_id);
      });
}

void MockAllocator::SetDebugClientInfo(SetDebugClientInfoRequestView request,
                                       SetDebugClientInfoCompleter::Sync& completer) {}

void MockAllocator::SetNewBufferCollectionConfig(
    const FakeBufferCollectionConfig& buffer_collection_config) {
  new_buffer_collection_config_ = buffer_collection_config;
}

// Returns the most recent created BufferCollection server.
// This may go out of scope if the caller releases the BufferCollection.
FakeBufferCollection* MockAllocator::GetMostRecentBufferCollection() const {
  return most_recent_buffer_collection_;
}

std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
MockAllocator::GetActiveBufferCollectionTokenClients() const {
  std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>> unowned_token_clients;
  unowned_token_clients.reserve(active_buffer_collections_.size());

  for (const auto& kv : active_buffer_collections_) {
    unowned_token_clients.push_back(kv.second.token_client);
  }
  return unowned_token_clients;
}

std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
MockAllocator::GetInactiveBufferCollectionTokenClients() const {
  std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>> unowned_token_clients;
  unowned_token_clients.reserve(inactive_buffer_collection_tokens_.size());

  for (const auto& token : inactive_buffer_collection_tokens_) {
    unowned_token_clients.push_back(token);
  }
  return unowned_token_clients;
}

void MockAllocator::NotImplemented_(const std::string& name, fidl::CompleterBase& completer) {
  ZX_PANIC("Not implemented: %s", name.c_str());
}

}  // namespace intel_display
