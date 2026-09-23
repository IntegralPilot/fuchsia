// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/lib/virtual_camera/virtual_camera.h"

#include <fuchsia/sysmem2/cpp/fidl.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/sys/cpp/component_context.h>
#include <zircon/status.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

class VirtualCameraTest : public gtest::RealLoopFixture {
 public:
  VirtualCameraTest() { context_ = sys::ComponentContext::CreateAndServeOutgoingDirectory(); }

 protected:
  virtual void SetUp() override {
    context_->svc()->Connect(allocator_.NewRequest());
    SetFailOnError(allocator_);

    fuchsia::sysmem2::AllocatorSetDebugClientInfoRequest set_debug_request;
    set_debug_request.set_name(fsl::GetCurrentProcessName() + "-client");
    set_debug_request.set_id(fsl::GetCurrentProcessKoid());
    allocator_->SetDebugClientInfo(std::move(set_debug_request));

    fidl::InterfaceHandle<fuchsia::sysmem2::Allocator> allocator;
    context_->svc()->Connect(allocator.NewRequest());
    auto result = camera::VirtualCamera::Create(std::move(allocator));
    ASSERT_TRUE(result.is_ok());
    virtual_camera_ = result.take_value();
  }

  virtual void TearDown() override {
    virtual_camera_ = nullptr;
    allocator_ = nullptr;
  }

  template <class T>
  static void SetFailOnError(fidl::InterfacePtr<T>& ptr, std::string name = T::Name_) {
    ptr.set_error_handler([=](zx_status_t status) {
      ADD_FAILURE() << name << " server disconnected: " << zx_status_get_string(status);
    });
  }

  void RunLoopUntilFailureOr(bool& condition) {
    RunLoopUntil([&]() { return HasFailure() || condition; });
  }

  std::unique_ptr<sys::ComponentContext> context_;
  fuchsia::sysmem2::AllocatorPtr allocator_;
  std::unique_ptr<camera::VirtualCamera> virtual_camera_;
};

TEST_F(VirtualCameraTest, FramesReceived) {
  fuchsia::camera3::DevicePtr camera;
  SetFailOnError(camera, "Camera");
  virtual_camera_->GetHandler()(camera.NewRequest());

  bool configurations_received = false;
  camera->GetConfigurations([&](std::vector<fuchsia::camera3::Configuration> configurations) {
    ASSERT_FALSE(configurations.empty());
    ASSERT_FALSE(configurations[0].streams.empty());
    configurations_received = true;
  });
  RunLoopUntilFailureOr(configurations_received);
  ASSERT_FALSE(HasFailure());

  fuchsia::camera3::StreamPtr stream;
  SetFailOnError(stream, "Stream");
  camera->ConnectToStream(0, stream.NewRequest());

  fuchsia::sysmem2::BufferCollectionTokenPtr token;
  fuchsia::sysmem2::AllocatorAllocateSharedCollectionRequest allocate_shared_request;
  allocate_shared_request.set_token_request(token.NewRequest());
  allocator_->AllocateSharedCollection(std::move(allocate_shared_request));

  token->Sync([&](fuchsia::sysmem2::Node_Sync_Result result) {
    stream->SetBufferCollection2(std::move(token));
  });
  fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> client_token;
  bool token_received = false;
  stream->WatchBufferCollection(
      [&](fidl::InterfaceHandle<fuchsia::sysmem::BufferCollectionToken> token_v1) {
        auto token_v2 = fuchsia::sysmem2::BufferCollectionTokenHandle(token_v1.TakeChannel());
        client_token = std::move(token_v2);
        token_received = true;
      });
  RunLoopUntilFailureOr(token_received);

  fuchsia::sysmem2::BufferCollectionPtr collection;
  SetFailOnError(collection, "BufferCollection");

  fuchsia::sysmem2::AllocatorBindSharedCollectionRequest bind_shared_request;
  bind_shared_request.set_token(std::move(client_token));
  bind_shared_request.set_buffer_collection_request(collection.NewRequest());
  allocator_->BindSharedCollection(std::move(bind_shared_request));

  fuchsia::sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  auto& constraints = *set_constraints_request.mutable_constraints();
  constraints.mutable_usage()->set_cpu(fuchsia::sysmem2::CPU_USAGE_READ);
  constraints.set_min_buffer_count_for_camping(2);
  collection->SetConstraints(std::move(set_constraints_request));

  fuchsia::sysmem2::BufferCollectionInfo buffers_received;
  bool buffers_allocated = false;
  collection->WaitForAllBuffersAllocated(
      [&](fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Result result) {
        ASSERT_TRUE(result.is_response());
        buffers_received = std::move(*result.response().mutable_buffer_collection_info());
        buffers_allocated = true;
      });
  RunLoopUntilFailureOr(buffers_allocated);
  ASSERT_FALSE(HasFailure());
  collection->Release();

  constexpr uint32_t kTargetFrameCount = 11;
  uint32_t frames_received = 0;
  bool all_frames_received = false;
  fit::function<void(fuchsia::camera3::FrameInfo)> check_frame;
  check_frame = [&](fuchsia::camera3::FrameInfo info) {
    fzl::VmoMapper mapper;
    ASSERT_EQ(
        mapper.Map(buffers_received.buffers()[info.buffer_index].vmo(), 0,
                   buffers_received.settings().buffer_settings().size_bytes(), ZX_VM_PERM_READ),
        ZX_OK);
    auto result = virtual_camera_->CheckFrame(mapper.start(), mapper.size(), info);
    EXPECT_TRUE(result.is_ok()) << result.error();
    result = virtual_camera_->CheckFrame(mapper.start(), 0, info);
    EXPECT_TRUE(result.is_error());
    mapper.Unmap();
    std::vector<char> zeros(0, static_cast<char>(mapper.size()));
    result = virtual_camera_->CheckFrame(zeros.data(), zeros.size(), info);
    EXPECT_TRUE(result.is_error());
    if (++frames_received < kTargetFrameCount) {
      stream->GetNextFrame(check_frame.share());
    } else {
      all_frames_received = true;
    }
  };
  stream->GetNextFrame(check_frame.share());
  RunLoopUntilFailureOr(all_frames_received);
  ASSERT_FALSE(HasFailure());
}
