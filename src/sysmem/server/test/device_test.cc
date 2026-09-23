// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem/cpp/natural_types.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/async-loop/default.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/fidl/cpp/wire/arena.h>
#include <lib/sync/completion.h>
#include <lib/zx/bti.h>
#include <stdlib.h>
#include <zircon/errors.h>

#include <gtest/gtest.h>

#include "src/sysmem/server/allocator.h"
#include "src/sysmem/server/buffer_collection.h"
#include "src/sysmem/server/logical_buffer_collection.h"
#include "src/sysmem/server/sysmem.h"
#include "src/sysmem/server/sysmem_config.h"

namespace sysmem_service {

namespace {

class FakeDdkSysmem : public ::testing::Test {
 public:
  FakeDdkSysmem() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}

  void SetUp() override {
    zx_status_t start_status = loop_.StartThread("FakeDdkSysmem");
    ZX_ASSERT_MSG(start_status == ZX_OK, "loop_.StartThread() failed: %s",
                  zx_status_get_string(start_status));

    libsync::Completion done;
    zx_status_t post_status = async::PostTask(loop_.dispatcher(), [this, &done]() mutable {
      sysmem_service::Sysmem::CreateArgs create_args;
      auto create_result = sysmem_service::Sysmem::Create(loop_.dispatcher(), create_args);
      ZX_ASSERT_MSG(create_result.is_ok(), "sysmem_service::Sysmem::Create() failed: %s",
                    create_result.status_string());
      device_ = std::move(create_result.value());
      done.Signal();
    });
    ZX_ASSERT_MSG(post_status == ZX_OK, "async::PostTask() failed: %s",
                  zx_status_get_string(post_status));
    done.Wait();
  }

  void TearDown() override {
    libsync::Completion done;
    zx_status_t post_status = async::PostTask(loop_.dispatcher(), [this, &done]() mutable {
      device_.reset();
      done.Signal();
    });
    ZX_ASSERT_MSG(post_status == ZX_OK, "async::PostTask() failed: %s",
                  zx_status_get_string(post_status));
    done.Wait();
  }

  fidl::ClientEnd<fuchsia_sysmem2::Allocator> Connect() {
    auto [client, server] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
    device_->SyncCall([this, server = std::move(server)]() mutable {
      sysmem_service::Allocator::CreateOwnedV2(std::move(server), device_.get(),
                                               device_->v2_allocators());
    });
    return std::move(client);
  }

  fidl::ClientEnd<fuchsia_sysmem2::BufferCollection> AllocateNonSharedCollection() {
    fidl::SyncClient<fuchsia_sysmem2::Allocator> allocator(Connect());

    auto [collection_client_end, collection_server_end] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();

    fuchsia_sysmem2::AllocatorAllocateNonSharedCollectionRequest allocate_non_shared_request;
    allocate_non_shared_request.collection_request() = std::move(collection_server_end);
    EXPECT_TRUE(
        allocator->AllocateNonSharedCollection(std::move(allocate_non_shared_request)).is_ok());
    return std::move(collection_client_end);
  }

 protected:
  async::Loop loop_;
  std::unique_ptr<sysmem_service::Sysmem> device_;
};

TEST_F(FakeDdkSysmem, Lifecycle) {
  // Queue up something that would be processed on the FIDL thread, so we can try to detect a
  // use-after-free if the FidlServer outlives the sysmem device.
  AllocateNonSharedCollection();
}

// Test that creating and tearing down a SecureMem connection works correctly.
TEST_F(FakeDdkSysmem, DummySecureMem) {
  auto [client, server] = fidl::Endpoints<fuchsia_sysmem2::SecureMem>::Create();

  device_->RunSyncOnClientDispatcher([this, client = std::move(client)]() mutable {
    std::lock_guard lock(device_->client_checker_);
    zx_status_t register_status = device_->RegisterSecureMemInternal(std::move(client));
    ZX_ASSERT_MSG(register_status == ZX_OK, "device_->RegisterSecureMemInternal() failed: %s",
                  zx_status_get_string(register_status));
  });

  device_->RunSyncOnClientDispatcher([this]() mutable {
    std::lock_guard lock(device_->client_checker_);
    // This shouldn't deadlock waiting for a message on the channel.
    zx_status_t unregister_status = device_->UnregisterSecureMemInternal();
    ZX_ASSERT_MSG(unregister_status == ZX_OK, "device_->UnregisterSecureMemInternal() failed: %s",
                  zx_status_get_string(unregister_status));
  });

  // This shouldn't cause a panic due to receiving peer closed.
  server.reset();
}

TEST_F(FakeDdkSysmem, NamedToken) {
  fidl::SyncClient<fuchsia_sysmem2::Allocator> allocator(Connect());

  auto [token_client_end, token_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();

  fuchsia_sysmem2::AllocatorAllocateSharedCollectionRequest allocate_shared_request;
  allocate_shared_request.token_request() = std::move(token_server_end);
  EXPECT_TRUE(allocator->AllocateSharedCollection(std::move(allocate_shared_request)).is_ok());

  fidl::SyncClient<fuchsia_sysmem2::BufferCollectionToken> token(std::move(token_client_end));

  // The buffer collection should end up with a name of "a" because that's the highest priority.
  {
    fuchsia_sysmem2::NodeSetNameRequest set_name_request;
    set_name_request.priority() = 5u;
    set_name_request.name() = "c";
    EXPECT_TRUE(token->SetName(std::move(set_name_request)).is_ok());
  }
  {
    fuchsia_sysmem2::NodeSetNameRequest set_name_request;
    set_name_request.priority() = 100u;
    set_name_request.name() = "a";
    EXPECT_TRUE(token->SetName(std::move(set_name_request)).is_ok());
  }
  {
    fuchsia_sysmem2::NodeSetNameRequest set_name_request;
    set_name_request.priority() = 6u;
    set_name_request.name() = "b";
    EXPECT_TRUE(token->SetName(std::move(set_name_request)).is_ok());
  }

  auto [collection_client_end, collection_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();

  fuchsia_sysmem2::AllocatorBindSharedCollectionRequest bind_shared_request;
  bind_shared_request.token() = token.TakeClientEnd();
  bind_shared_request.buffer_collection_request() = std::move(collection_server_end);
  EXPECT_TRUE(allocator->BindSharedCollection(std::move(bind_shared_request)).is_ok());

  // Poll until a matching buffer collection is found.
  while (true) {
    bool found_collection = device_->SyncCall([&]() {
      if (device_->logical_buffer_collections().size() == 1) {
        const auto* logical_collection = *device_->logical_buffer_collections().begin();
        auto collection_views = logical_collection->collection_views();
        if (collection_views.size() == 1) {
          auto name = logical_collection->name();
          EXPECT_TRUE(name);
          EXPECT_EQ("a", *name);
          return true;
        }
      }
      return false;
    });
    if (found_collection) {
      break;
    }
  }
}

TEST_F(FakeDdkSysmem, NamedClient) {
  auto collection_client_end = AllocateNonSharedCollection();

  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection(std::move(collection_client_end));
  fuchsia_sysmem2::NodeSetDebugClientInfoRequest set_debug_request;
  set_debug_request.name() = "a";
  set_debug_request.id() = 5;
  EXPECT_TRUE(collection->SetDebugClientInfo(std::move(set_debug_request)).is_ok());

  // Poll until a matching buffer collection is found.
  while (true) {
    bool found_collection = device_->SyncCall([&]() mutable {
      if (device_->logical_buffer_collections().size() == 1) {
        const auto* logical_collection = *device_->logical_buffer_collections().begin();
        auto collection_views = logical_collection->collection_views();
        if (collection_views.size() == 1) {
          const BufferCollection* collection = logical_collection->collection_views().front();
          if (collection->node_properties().client_debug_info().name == "a") {
            EXPECT_EQ(5u, collection->node_properties().client_debug_info().id);
            return true;
          }
        }
      }
      return false;
    });
    if (found_collection) {
      break;
    }
  }
}

// Check that the allocator name overrides the collection name.
TEST_F(FakeDdkSysmem, NamedAllocatorToken) {
  fidl::SyncClient<fuchsia_sysmem2::Allocator> allocator(Connect());

  auto [token_client_end, token_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();

  fuchsia_sysmem2::AllocatorAllocateSharedCollectionRequest allocate_shared_request;
  allocate_shared_request.token_request() = std::move(token_server_end);
  EXPECT_TRUE(allocator->AllocateSharedCollection(std::move(allocate_shared_request)).is_ok());

  fidl::SyncClient<fuchsia_sysmem2::BufferCollectionToken> token(std::move(token_client_end));

  const char kAlphabetString[] = "abcdefghijklmnopqrstuvwxyz";
  {
    fuchsia_sysmem2::AllocatorSetDebugClientInfoRequest set_debug_request;
    set_debug_request.name() = kAlphabetString;
    set_debug_request.id() = 5;
    EXPECT_TRUE(allocator->SetDebugClientInfo(std::move(set_debug_request)).is_ok());
  }
  // Despite this message being sent after the above message, this message is not the "final word"
  // on the debug info, because the allocator will fence all token messages before transferring
  // the allocator's debug info to the BufferColllection.
  {
    fuchsia_sysmem2::NodeSetDebugClientInfoRequest set_debug_request;
    set_debug_request.name() = "bad";
    set_debug_request.id() = 6;
    EXPECT_TRUE(token->SetDebugClientInfo(std::move(set_debug_request)).is_ok());
  }

  auto [collection_client_end, collection_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();

  fuchsia_sysmem2::AllocatorBindSharedCollectionRequest bind_shared_request;
  bind_shared_request.token() = token.TakeClientEnd();
  bind_shared_request.buffer_collection_request() = std::move(collection_server_end);
  EXPECT_TRUE(allocator->BindSharedCollection(std::move(bind_shared_request)).is_ok());

  // Poll until a matching buffer collection is found. If this gets stuck, sysmem may be failing
  // to ensure that the allocator's debug info is the "last word" - may be failing to fence the
  // token messages before applyign the allocator's debug info to the token.
  while (true) {
    bool found_collection = device_->SyncCall([&]() {
      if (device_->logical_buffer_collections().size() == 1) {
        const auto* logical_collection = *device_->logical_buffer_collections().begin();
        auto collection_views = logical_collection->collection_views();
        if (collection_views.size() == 1) {
          const auto& collection = collection_views.front();
          // This needs to tell the difference between "abcdefghijklmnopqrstuvwxyz" and
          // "bad (was abcdefghijklmnopqrstuvwxyz)".
          if (collection->node_properties().client_debug_info().name.find(kAlphabetString) == 0) {
            EXPECT_EQ(5u, collection->node_properties().client_debug_info().id);
            return true;
          }
        }
      }
      return false;
    });
    if (found_collection) {
      break;
    }
  }
}

TEST_F(FakeDdkSysmem, MaxSize) {
  device_->set_settings(sysmem_service::Settings{.max_allocation_size = zx_system_get_page_size()});

  auto collection_client = AllocateNonSharedCollection();

  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  constraints.min_buffer_count() = 1;
  auto& bmc = constraints.buffer_memory_constraints().emplace();
  bmc.min_size_bytes() = zx_system_get_page_size() * 2;
  bmc.cpu_domain_supported() = true;
  constraints.usage().emplace().cpu() = fuchsia_sysmem::kCpuUsageRead;

  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection(std::move(collection_client));
  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  set_constraints_request.constraints() = std::move(constraints);
  EXPECT_TRUE(collection->SetConstraints(std::move(set_constraints_request)).is_ok());

  // Sysmem should fail the collection and return an error.
  auto wait_result = collection->WaitForAllBuffersAllocated();
  EXPECT_TRUE(!wait_result.is_ok());
}

// Check that teardown doesn't leak any memory (detected through LSAN).
TEST_F(FakeDdkSysmem, TeardownLeak) {
  auto collection_client = AllocateNonSharedCollection();

  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  constraints.min_buffer_count() = 1;
  auto& bmc = constraints.buffer_memory_constraints().emplace();
  bmc.min_size_bytes() = zx_system_get_page_size();
  bmc.cpu_domain_supported() = true;
  constraints.usage().emplace().cpu() = fuchsia_sysmem::kCpuUsageRead;

  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection(std::move(collection_client));
  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  set_constraints_request.constraints() = std::move(constraints);
  EXPECT_TRUE(collection->SetConstraints(std::move(set_constraints_request)).is_ok());

  auto wait_result = collection->WaitForAllBuffersAllocated();
  EXPECT_TRUE(wait_result.is_ok());
  auto info = std::move(wait_result->buffer_collection_info());

  for (uint32_t i = 0; i < info->buffers()->size(); i++) {
    info->buffers()->at(i).vmo().reset();
  }
  collection = {};
}

// Check that there are no circular references from a VMO to the logical buffer collection.
TEST_F(FakeDdkSysmem, BufferLeak) {
  auto collection_client = AllocateNonSharedCollection();

  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  constraints.min_buffer_count() = 1;
  auto& bmc = constraints.buffer_memory_constraints().emplace();
  bmc.min_size_bytes() = zx_system_get_page_size();
  bmc.cpu_domain_supported() = true;
  constraints.usage().emplace().cpu() = fuchsia_sysmem::kCpuUsageRead;

  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection(std::move(collection_client));
  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  set_constraints_request.constraints() = std::move(constraints);
  EXPECT_TRUE(collection->SetConstraints(std::move(set_constraints_request)).is_ok());

  auto wait_result = collection->WaitForAllBuffersAllocated();
  EXPECT_TRUE(wait_result.is_ok());
  auto info = std::move(wait_result->buffer_collection_info());

  for (uint32_t i = 0; i < info->buffers()->size(); i++) {
    info->buffers()->at(i).vmo().reset();
  }

  collection = {};

  // Poll until all buffer collections are deleted.
  while (true) {
    bool no_collections = device_->SyncCall([&]() {
      no_collections = device_->logical_buffer_collections().empty();
      return no_collections;
    });
    if (no_collections) {
      break;
    }
  }
}

}  // namespace
}  // namespace sysmem_service
