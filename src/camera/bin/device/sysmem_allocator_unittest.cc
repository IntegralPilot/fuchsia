// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/bin/device/sysmem_allocator.h"

#include <fuchsia/sysmem2/cpp/fidl_test_base.h>
#include <lib/async/cpp/executor.h>
#include <lib/fidl/cpp/binding.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace camera {
namespace {

class FakeBufferCollection : public fuchsia::sysmem2::testing::BufferCollection_TestBase {
 public:
  FakeBufferCollection(fidl::InterfaceRequest<fuchsia::sysmem2::BufferCollection> request)
      : binding_{this, std::move(request)} {}

  void PeerClose() { binding_.Unbind(); }
  void CompleteBufferAllocation(
      fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Result result) {
    ASSERT_TRUE(allocated_callback_);
    allocated_callback_(std::move(result));
    allocated_callback_ = nullptr;
  }

 private:
  // |fuchsia::sysmem2::BufferCollection|
  void Release() override { binding_.Close(ZX_OK); }
  void SetName(fuchsia::sysmem2::NodeSetNameRequest request) override {}
  void SetConstraints(fuchsia::sysmem2::BufferCollectionSetConstraintsRequest request) override {}
  void WaitForAllBuffersAllocated(WaitForAllBuffersAllocatedCallback callback) override {
    allocated_callback_ = std::move(callback);
  }

  void AttachLifetimeTracking(
      fuchsia::sysmem2::BufferCollectionAttachLifetimeTrackingRequest request) override {
    lifetime_tracking_ = std::move(*request.mutable_server_end());
  }

  void NotImplemented_(const std::string& name) override {
    FAIL() << "Not Implemented BufferCollection." << name;
  }

  fidl::Binding<fuchsia::sysmem2::BufferCollection> binding_;
  WaitForAllBuffersAllocatedCallback allocated_callback_;
  zx::eventpair lifetime_tracking_;
};

class FakeAllocator : public fuchsia::sysmem2::testing::Allocator_TestBase {
 public:
  fuchsia::sysmem2::AllocatorHandle NewBinding() { return binding_.NewBinding(); }

  const std::vector<std::unique_ptr<FakeBufferCollection>>& bound_collections() const {
    return collections_;
  }

 private:
  // |fuchsia::sysmem2::Allocator|
  void AllocateNonSharedCollection(
      fuchsia::sysmem2::AllocatorAllocateNonSharedCollectionRequest request) override {
    collections_.emplace_back(
        std::make_unique<FakeBufferCollection>(std::move(*request.mutable_collection_request())));
  }
  void BindSharedCollection(
      fuchsia::sysmem2::AllocatorBindSharedCollectionRequest request) override {
    collections_.emplace_back(std::make_unique<FakeBufferCollection>(
        std::move(*request.mutable_buffer_collection_request())));
  }

  void NotImplemented_(const std::string& name) override {
    FAIL() << "Not Implemented Allocator." << name;
  }

  fidl::Binding<fuchsia::sysmem2::Allocator> binding_{this};
  std::vector<std::unique_ptr<FakeBufferCollection>> collections_;
};

class SysmemAllocatorTest : public gtest::TestLoopFixture {
 protected:
  FakeBufferCollection* last_bound_collection() {
    return sysmem_allocator_.bound_collections().empty()
               ? nullptr
               : sysmem_allocator_.bound_collections().back().get();
  }

  FakeAllocator sysmem_allocator_;
  SysmemAllocator allocator_{sysmem_allocator_.NewBinding()};
  async::Executor executor_{dispatcher()};
};

TEST_F(SysmemAllocatorTest, BindSharedCollection) {
  fuchsia::sysmem2::BufferCollectionTokenHandle token;
  auto request = token.NewRequest();
  fpromise::result<BufferCollectionWithLifetime, zx_status_t> result;
  executor_.schedule_task(
      allocator_.BindSharedCollection(std::move(token), {}, "CollectionName")
          .then([&result](fpromise::result<BufferCollectionWithLifetime, zx_status_t>& r) mutable {
            result = std::move(r);
          }));
  RunLoopUntilIdle();

  // Now we should have the shared buffer collection.
  ASSERT_EQ(1u, sysmem_allocator_.bound_collections().size());
  fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Result wait_result;
  fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Response wait_response;
  wait_response.set_buffer_collection_info(fuchsia::sysmem2::BufferCollectionInfo{});
  wait_result.set_response(std::move(wait_response));
  last_bound_collection()->CompleteBufferAllocation(std::move(wait_result));
  RunLoopUntilIdle();
  EXPECT_TRUE(result.is_ok());
}

}  // namespace
}  // namespace camera
