// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TESTING_MOCK_ALLOCATOR_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TESTING_MOCK_ALLOCATOR_H_

#include <fidl/fuchsia.sysmem2/cpp/wire_test_base.h>
#include <lib/async/dispatcher.h>

#include <memory>
#include <unordered_map>

#include "src/graphics/display/drivers/intel-display/testing/fake-buffer-collection.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"

namespace intel_display {

class MockAllocator : public fidl::testing::WireTestBase<fuchsia_sysmem2::Allocator> {
 public:
  explicit MockAllocator(async_dispatcher_t* dispatcher);
  ~MockAllocator() override;

  // fuchsia_sysmem2::Allocator:
  void BindSharedCollection(BindSharedCollectionRequestView request,
                            BindSharedCollectionCompleter::Sync& completer) override;
  void SetDebugClientInfo(SetDebugClientInfoRequestView request,
                          SetDebugClientInfoCompleter::Sync& completer) override;
  void SetNewBufferCollectionConfig(const FakeBufferCollectionConfig& buffer_collection_config);

  // fidl::testing::WireTestBase:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override;

  // Returns the most recent created BufferCollection server.
  // This may go out of scope if the caller releases the BufferCollection.
  FakeBufferCollection* GetMostRecentBufferCollection() const;

  std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
  GetActiveBufferCollectionTokenClients() const;

  std::vector<fidl::UnownedClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
  GetInactiveBufferCollectionTokenClients() const;

 private:
  struct BufferCollection {
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_client;
    std::unique_ptr<FakeBufferCollection> mock_buffer_collection;
  };

  FakeBufferCollectionConfig new_buffer_collection_config_;
  FakeBufferCollection* most_recent_buffer_collection_ = nullptr;
  std::unordered_map<display::DriverBufferCollectionId, BufferCollection>
      active_buffer_collections_;
  std::vector<fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>>
      inactive_buffer_collection_tokens_;

  display::DriverBufferCollectionId next_buffer_collection_id_ =
      display::DriverBufferCollectionId(0);

  async_dispatcher_t* dispatcher_ = nullptr;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_TESTING_MOCK_ALLOCATOR_H_
