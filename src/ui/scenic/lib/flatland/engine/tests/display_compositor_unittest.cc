// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/engine/display_compositor.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/time.h>

#include <memory>
#include <span>

#include <gmock/gmock.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"
#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/allocation/id.h"
#include "src/ui/scenic/lib/display/util.h"
#include "src/ui/scenic/lib/flatland/engine/tests/mock_display_coordinator.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/renderer/mock_renderer.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/tests/utils/promise.h"

using ::testing::_;
using ::testing::Eq;
using ::testing::Return;

using allocation::BufferCollectionUsage;
using allocation::ImageMetadata;
using flatland::MockDisplayCoordinator;
using flatland::SrcToDest;

using fuchsia_ui_composition::ImageFlip;
using fuchsia_ui_composition::Orientation;
using integration_tests::ReturnPromise;

namespace fuchsia_hardware_display::wire {

bool operator==(const BufferCollectionId& a, const BufferCollectionId& b) {
  return a.value == b.value;
}

bool operator==(const ImageId& a, const ImageId& b) { return a.value == b.value; }

bool operator==(const LayerId& a, const LayerId& b) { return a.value == b.value; }

}  // namespace fuchsia_hardware_display::wire

namespace fuchsia_hardware_display_types::wire {

bool operator==(const DisplayId& a, const DisplayId& b) { return a.value == b.value; }

}  // namespace fuchsia_hardware_display_types::wire

namespace fuchsia_math::wire {

bool operator==(const RectU& a, const RectU& b) {
  return a.x == b.x && a.y == b.y && a.width == b.width && a.height == b.height;
}

}  // namespace fuchsia_math::wire

namespace flatland::test {

namespace {

constexpr uint32_t kMaxDisplayLayersCount = 2;

// Returns a matcher matching the `field` from [`fuchsia.hardware.display/Coordinator.FunctionName`]
// FIDL request.
#define MatchRequestField(FunctionName, field, matcher)                                          \
  testing::Field(&fidl::WireRequest<fuchsia_hardware_display::Coordinator::FunctionName>::field, \
                 (matcher))

fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> DuplicateToken(
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>& token) {
  fidl::Arena arena;
  uint32_t mask = ZX_RIGHT_SAME_RIGHTS;
  auto result = fidl::WireCall(token)->DuplicateSync(
      fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateSyncRequest::Builder(arena)
          .rights_attenuation_masks(fidl::VectorView<uint32_t>::FromExternal(&mask, 1))
          .Build());
  FX_CHECK(result.ok());
  FX_CHECK(result->tokens().size() == 1u);
  return std::move(result->tokens()[0]);
}

void SetConstraintsAndClose(fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
                            fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token,
                            fuchsia_sysmem2::BufferCollectionConstraints constraints) {
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(token))
          .buffer_collection_request(std::move(server_end))
          .Build());
  ASSERT_TRUE(result.ok());

  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> collection(std::move(client_end));
  auto set_result = collection->SetConstraints(
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
          .constraints(fidl::ToWire(arena, std::move(constraints)))
          .Build());
  ASSERT_TRUE(set_result.ok());
  auto release_result = collection->Release();
  ASSERT_TRUE(release_result.ok());
}

bool RunWithTimeoutOrUntil(fit::function<bool()> condition, zx::duration timeout,
                           zx::duration step) {
  zx::duration wait_time = zx::msec(0);
  while (wait_time <= timeout) {
    if (condition())
      return true;
    zx::nanosleep(zx::deadline_after(step));
    wait_time += step;
  }

  return condition();
}

}  // namespace

class DisplayCompositorTest : public gtest::RealLoopFixture {
 public:
  void SetUp() override {
    gtest::RealLoopFixture::SetUp();
    async_set_default_dispatcher(dispatcher());

    sysmem_allocator_ = utils::CreateSysmemAllocatorClient(dispatcher(), "DisplayCompositorTest");

    renderer_ = std::make_shared<flatland::MockRenderer>();

    zx::result endpoints_result = fidl::CreateEndpoints<fuchsia_hardware_display::Coordinator>();
    FX_CHECK(endpoints_result.is_ok())
        << "Failed to create FIDL endpoints for the display coordinator: "
        << endpoints_result.status_string();
    auto [coordinator_client, coordinator_server] = std::move(endpoints_result).value();

    mock_display_coordinator_ =
        std::make_unique<testing::StrictMock<flatland::MockDisplayCoordinator>>();
    // The fidl::Server requires the binding and teardown to occur on the
    // same thread where the FIDL server runs.
    libsync::Completion completion;
    async::PostTask(
        display_coordinator_loop_.dispatcher(),
        [this, &completion, coordinator_server = std::move(coordinator_server)]() mutable {
          mock_display_coordinator_->Bind(std::move(coordinator_server),
                                          display_coordinator_loop_.dispatcher());
          completion.Signal();
        });
    display_coordinator_loop_.StartThread("display-coordinator-loop");
    completion.Wait();

    auto coordinator_proxy =
        std::make_shared<display::CoordinatorProxy>(std::move(coordinator_client), dispatcher());

    display_compositor_ = std::make_shared<flatland::DisplayCompositor>(
        dispatcher(), std::move(coordinator_proxy), renderer_,
        utils::CreateSysmemAllocatorClient(dispatcher(), "display_compositor_unittest"),
        flatland::DisplayCompositorConfig{});
  }

  void TearDown() override {
    renderer_.reset();
    display_compositor_.reset();

    // This is to make sure that the display coordinator loop has finished
    // handling all its pending tasks.
    RunLoopUntilIdle();
    libsync::Completion completion;
    async::PostTask(display_coordinator_loop_.dispatcher(), [this, &completion]() mutable {
      display_coordinator_loop_.RunUntilIdle();
      mock_display_coordinator_.reset();
      completion.Signal();
    });
    completion.Wait();
    display_coordinator_loop_.Quit();
    display_coordinator_loop_.JoinThreads();

    gtest::RealLoopFixture::TearDown();
  }

  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> CreateToken() {
    auto [client_end, server_end] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    fidl::Arena arena;
    fidl::OneWayStatus result = sysmem_allocator_->AllocateSharedCollection(
        fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena)
            .token_request(std::move(server_end))
            .Build());
    FX_DCHECK(result.ok());
    auto sync_result = fidl::WireCall(client_end)->Sync();
    FX_DCHECK(sync_result.ok());
    return std::move(client_end);
  }

  void SetDisplaySupported(allocation::GlobalBufferCollectionId id, bool is_supported) {
    std::scoped_lock lock(display_compositor_->lock_);
    display_compositor_->buffer_collection_supports_display_[id] = is_supported;
    display_compositor_->buffer_collection_tiling_type_map_[id] =
        fuchsia_hardware_display_types::kImageTilingTypeLinear;
  }

  void ForceRendererOnlyMode(bool force_renderer_only) {
    const_cast<bool&>(display_compositor_->config_.enable_direct_to_display) = !force_renderer_only;
  }

  void SendOnVsyncEvent(display::WireConfigStamp stamp) {
    display_compositor_->OnVsync(zx::time_monotonic(), stamp);
  }

  std::deque<DisplayCompositor::ApplyConfigInfo> GetPendingApplyConfigs() {
    return display_compositor_->pending_apply_configs_;
  }

  bool BufferCollectionSupportsDisplay(allocation::GlobalBufferCollectionId id) {
    std::scoped_lock lock(display_compositor_->lock_);
    return display_compositor_->buffer_collection_supports_display_.contains(id) &&
           display_compositor_->buffer_collection_supports_display_[id];
  }

  bool TryDirectToDisplay(std::span<const RenderData> render_data_list) {
    std::scoped_lock lock(display_compositor_->lock_);
    return display_compositor_->TryDirectToDisplay(render_data_list, /* frame_number= */ 1,
                                                   /* trace_flow_id= */ 1);
  }

 protected:
  bool RunPromise(fpromise::promise<> promise) {
    return integration_tests::RunPromise(
        dispatcher(), [this] { RunLoopUntilIdle(); }, std::move(promise));
  }

  static constexpr fuchsia_images2::PixelFormat kPixelFormat =
      fuchsia_images2::PixelFormat::kB8G8R8A8;

  async::Loop display_coordinator_loop_{&kAsyncLoopConfigNeverAttachToThread};
  std::unique_ptr<flatland::MockDisplayCoordinator> mock_display_coordinator_;
  std::shared_ptr<flatland::MockRenderer> renderer_;
  std::shared_ptr<flatland::DisplayCompositor> display_compositor_;

  // Only for use on the main thread. Establish a new connection when on the MockDisplayCoordinator
  // thread.
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;

  void HardwareFrameCorrectnessWithRotationTester(
      Orientation orientation, ImageFlip image_flip, fuchsia_math::wire::RectU expected_dst,
      display::WireCoordinateTransformation expected_transform);
};

// TODO(https://fxbug.dev/324688770): Dispatch all DisplayCompositor methods
// to the test loop.

TEST_F(DisplayCompositorTest, ImportAndReleaseBufferCollectionTest) {
  constexpr allocation::GlobalBufferCollectionId kGlobalBufferCollectionId = 15;
  const display::WireBufferCollectionId kDisplayBufferCollectionId =
      display::ToDisplayFidlBufferCollectionId(kGlobalBufferCollectionId);

  EXPECT_CALL(*mock_display_coordinator_,
              ImportBufferCollection(MatchRequestField(ImportBufferCollection, buffer_collection_id,
                                                       Eq(kDisplayBufferCollectionId)),
                                     _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorImportBufferCollectionRequest*,
             MockDisplayCoordinator::ImportBufferCollectionCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));

  EXPECT_CALL(*mock_display_coordinator_,
              SetBufferCollectionConstraints(
                  MatchRequestField(SetBufferCollectionConstraints, buffer_collection_id,
                                    Eq(kDisplayBufferCollectionId)),
                  _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorSetBufferCollectionConstraintsRequest*,
             MockDisplayCoordinator::SetBufferCollectionConstraintsCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));

  // Save token to avoid early token failure.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_ref;
  EXPECT_CALL(*renderer_, ImportBufferCollection(kGlobalBufferCollectionId, _, _, _, _))
      .WillOnce([&token_ref](allocation::GlobalBufferCollectionId,
                             fidl::WireClient<fuchsia_sysmem2::Allocator>&,
                             fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> renderer_token,
                             BufferCollectionUsage, std::optional<fuchsia_math::SizeU>) {
        token_ref = std::move(renderer_token);
        return fpromise::make_ok_promise();
      });
  EXPECT_TRUE(RunPromise(display_compositor_->ImportBufferCollection(
      kGlobalBufferCollectionId, sysmem_allocator_,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>{CreateToken()},
      BufferCollectionUsage::kClientImage, std::nullopt)));

  EXPECT_CALL(
      *mock_display_coordinator_,
      ReleaseBufferCollection(MatchRequestField(ReleaseBufferCollection, buffer_collection_id,
                                                Eq(kDisplayBufferCollectionId)),
                              _))
      .Times(1)
      .WillOnce(Return());

  EXPECT_CALL(*renderer_, ReleaseBufferCollection(kGlobalBufferCollectionId, _)).WillOnce(Return());
  display_compositor_->ReleaseBufferCollection(kGlobalBufferCollectionId,
                                               BufferCollectionUsage::kClientImage);

  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());
}

// This test makes sure the buffer negotiations work as intended.
// There are three participants: the client, the display and the renderer.
// Each participant sets {min_buffer_count, max_buffer_count} constraints like so:
// Client: {1, 3}
// Display: {2, 3}
// Renderer: {1, 2}
// Since 2 is the only valid overlap between all of them we expect 2 buffers to be allocated.
TEST_F(DisplayCompositorTest,
       SysmemNegotiationTest_WhenDisplayConstraintsCompatible_TheyShouldBeIncluded) {
  // Create two tokens: one for acting as the "client" and inspecting allocation results with, and
  // one to send to the display compositor.
  auto client_token = CreateToken();
  auto compositor_token = DuplicateToken(client_token);

  // Set "client" constraints.
  auto [client_collection_client_end, client_collection_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator_->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(client_token))
          .buffer_collection_request(std::move(client_collection_server_end))
          .Build());
  ASSERT_TRUE(result.ok());
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> client_collection(
      std::move(client_collection_client_end));

  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  fuchsia_sysmem2::BufferUsage usage;
  usage.cpu(fuchsia_sysmem2::kCpuUsageWrite);
  constraints.usage(std::move(usage));
  constraints.min_buffer_count(1);
  constraints.max_buffer_count(3);
  fuchsia_sysmem2::BufferMemoryConstraints bmc;
  bmc.min_size_bytes(1);
  bmc.max_size_bytes(20);
  constraints.buffer_memory_constraints(std::move(bmc));
  fuchsia_sysmem2::ImageFormatConstraints ifc;
  ifc.pixel_format(fuchsia_images2::PixelFormat::kB8G8R8A8);
  ifc.color_spaces({{fuchsia_images2::ColorSpace::kSrgb}});
  ifc.min_size(fuchsia_math::SizeU(1, 1));
  constraints.image_format_constraints({{std::move(ifc)}});
  set_constraints_request.constraints(std::move(constraints));
  auto set_constraints_result =
      client_collection->SetConstraints(std::move(set_constraints_request));
  ASSERT_TRUE(set_constraints_result.is_ok());

  const auto kGlobalBufferCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const display::WireBufferCollectionId kDisplayBufferCollectionId =
      display::ToDisplayFidlBufferCollectionId(kGlobalBufferCollectionId);

  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> display_token;
  EXPECT_CALL(*mock_display_coordinator_,
              ImportBufferCollection(MatchRequestField(ImportBufferCollection, buffer_collection_id,
                                                       Eq(kDisplayBufferCollectionId)),
                                     _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [&display_token](
              fuchsia_hardware_display::wire::CoordinatorImportBufferCollectionRequest* request,
              MockDisplayCoordinator::ImportBufferCollectionCompleter::Sync& completer) {
            display_token = std::move(request->buffer_collection_token);
            completer.Reply(fit::ok());
          }));

  // Set display constraints.
  EXPECT_CALL(*mock_display_coordinator_,
              SetBufferCollectionConstraints(
                  MatchRequestField(SetBufferCollectionConstraints, buffer_collection_id,
                                    Eq(kDisplayBufferCollectionId)),
                  _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [this, &display_token](
              fuchsia_hardware_display::wire::CoordinatorSetBufferCollectionConstraintsRequest*,
              MockDisplayCoordinator::SetBufferCollectionConstraintsCompleter::Sync& completer) {
            fuchsia_sysmem2::BufferCollectionConstraints constraints;
            fuchsia_sysmem2::BufferUsage usage;
            usage.cpu(fuchsia_sysmem2::kCpuUsageWrite);
            constraints.usage(std::move(usage));
            constraints.min_buffer_count(2);
            constraints.max_buffer_count(3);

            async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
            auto sysmem_allocator =
                utils::CreateSysmemAllocatorClient(dispatcher(), "MockDisplayCoordinator");
            SetConstraintsAndClose(sysmem_allocator, std::move(display_token),
                                   std::move(constraints));
            loop.RunUntilIdle();
            completer.Reply(fit::ok());
          }));

  EXPECT_CALL(*mock_display_coordinator_,
              ImportImage(testing::AllOf(MatchRequestField(ImportImage, buffer_collection_id,
                                                           Eq(kDisplayBufferCollectionId)),
                                         MatchRequestField(ImportImage, buffer_index, Eq(0))),
                          _))
      .Times(1)
      .WillOnce(testing::Invoke([](fuchsia_hardware_display::wire::CoordinatorImportImageRequest*,
                                   MockDisplayCoordinator::ImportImageCompleter::Sync& completer) {
        completer.Reply(fit::ok());
      }));

  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());

  // Set renderer constraints.
  EXPECT_CALL(*renderer_, ImportBufferCollection(kGlobalBufferCollectionId, _, _, _, _))
      .WillOnce([this](auto id, auto& client, auto renderer_token, auto usage, auto size) {
        fuchsia_sysmem2::BufferCollectionConstraints constraints;
        fuchsia_sysmem2::BufferUsage buf_usage;
        buf_usage.cpu(fuchsia_sysmem2::kCpuUsageWrite);
        constraints.usage(std::move(buf_usage));
        constraints.min_buffer_count(1);
        constraints.max_buffer_count(2);
        SetConstraintsAndClose(sysmem_allocator_, std::move(renderer_token),
                               std::move(constraints));
        return fpromise::make_ok_promise();
      });

  ASSERT_TRUE(RunPromise(display_compositor_->ImportBufferCollection(
      kGlobalBufferCollectionId, sysmem_allocator_, std::move(compositor_token),
      BufferCollectionUsage::kClientImage, std::nullopt)));

  {
    auto wait_result = client_collection->WaitForAllBuffersAllocated();
    ASSERT_TRUE(wait_result.is_ok());
    EXPECT_EQ(wait_result->buffer_collection_info().value().buffers().value().size(), 2u);
  }

  // ImportBufferImage() to confirm that the allocation was handled correctly.
  EXPECT_CALL(*renderer_, ImportBufferImage(_, _)).WillOnce(ReturnPromise(fpromise::ok()));
  ASSERT_TRUE(RunPromise(display_compositor_->ImportBufferImage(
      ImageMetadata{.collection_id = kGlobalBufferCollectionId,
                    .identifier = display::ImageId(1),
                    .vmo_index = 0,
                    .width = 1,
                    .height = 1},
      BufferCollectionUsage::kClientImage)));
  EXPECT_TRUE(BufferCollectionSupportsDisplay(kGlobalBufferCollectionId));
}

// This test makes sure the buffer negotiations work as intended.
// There are three participants: the client, the display and the renderer.
// Each participant sets {min_buffer_count, max_buffer_count} constraints like so:
// Client: {1, 2}
// Display: {1, 1}
// Renderer: {2, 2}
// Since there is no valid overlap between all participants the display should drop out and we
// expect 2 buffers to be allocated (the only valid overlap between client and renderer).
TEST_F(DisplayCompositorTest,
       SysmemNegotiationTest_WhenDisplayConstraintsIncompatible_TheyShouldBeExcluded) {
  // Create two tokens: one for acting as the "client" and inspecting allocation results with, and
  // one to send to the display compositor.
  auto client_token = CreateToken();
  auto compositor_token = DuplicateToken(client_token);

  // Set "client" constraints.
  auto [client_collection_client_end, client_collection_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator_->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(client_token))
          .buffer_collection_request(std::move(client_collection_server_end))
          .Build());
  ASSERT_TRUE(result.ok());
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> client_collection(
      std::move(client_collection_client_end));

  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  fuchsia_sysmem2::BufferUsage usage;
  usage.cpu(fuchsia_sysmem2::kCpuUsageWrite);
  constraints.usage(std::move(usage));
  constraints.min_buffer_count(1);
  constraints.max_buffer_count(2);
  fuchsia_sysmem2::BufferMemoryConstraints bmc;
  bmc.min_size_bytes(1);
  bmc.max_size_bytes(20);
  constraints.buffer_memory_constraints(std::move(bmc));
  fuchsia_sysmem2::ImageFormatConstraints ifc;
  ifc.pixel_format(fuchsia_images2::PixelFormat::kB8G8R8A8);
  ifc.color_spaces({{fuchsia_images2::ColorSpace::kSrgb}});
  ifc.min_size(fuchsia_math::SizeU(1, 1));
  constraints.image_format_constraints({{std::move(ifc)}});
  set_constraints_request.constraints(std::move(constraints));
  auto set_constraints_result =
      client_collection->SetConstraints(std::move(set_constraints_request));
  ASSERT_TRUE(set_constraints_result.is_ok());

  const auto kGlobalBufferCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const display::WireBufferCollectionId kDisplayBufferCollectionId =
      display::ToDisplayFidlBufferCollectionId(kGlobalBufferCollectionId);

  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> display_token;
  EXPECT_CALL(*mock_display_coordinator_,
              ImportBufferCollection(MatchRequestField(ImportBufferCollection, buffer_collection_id,
                                                       Eq(kDisplayBufferCollectionId)),
                                     _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [&display_token](
              fuchsia_hardware_display::wire::CoordinatorImportBufferCollectionRequest* request,
              MockDisplayCoordinator::ImportBufferCollectionCompleter::Sync& completer) {
            display_token = std::move(request->buffer_collection_token);
            completer.Reply(fit::ok());
          }));

  // Set display constraints.
  EXPECT_CALL(*mock_display_coordinator_,
              SetBufferCollectionConstraints(
                  MatchRequestField(SetBufferCollectionConstraints, buffer_collection_id,
                                    Eq(kDisplayBufferCollectionId)),
                  _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [this, &display_token](
              fuchsia_hardware_display::wire::CoordinatorSetBufferCollectionConstraintsRequest*,
              MockDisplayCoordinator::SetBufferCollectionConstraintsCompleter::Sync& completer) {
            fuchsia_sysmem2::BufferCollectionConstraints constraints;
            fuchsia_sysmem2::BufferUsage usage;
            usage.cpu(fuchsia_sysmem2::kCpuUsageWrite);
            constraints.usage(std::move(usage));
            constraints.min_buffer_count(1);
            constraints.max_buffer_count(1);

            async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
            auto sysmem_allocator =
                utils::CreateSysmemAllocatorClient(dispatcher(), "MockDisplayCoordinator");
            SetConstraintsAndClose(sysmem_allocator, std::move(display_token),
                                   std::move(constraints));
            loop.RunUntilIdle();
            completer.Reply(fit::ok());
          }));

  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());

  // Set renderer constraints.
  EXPECT_CALL(*renderer_, ImportBufferCollection(kGlobalBufferCollectionId, _, _, _, _))
      .WillOnce([this](auto id, auto& client, auto renderer_token, auto usage, auto size) {
        fuchsia_sysmem2::BufferCollectionConstraints constraints;
        fuchsia_sysmem2::BufferUsage buf_usage;
        buf_usage.cpu(fuchsia_sysmem2::kCpuUsageWrite);
        constraints.usage(std::move(buf_usage));
        constraints.min_buffer_count(2);
        constraints.max_buffer_count(2);
        SetConstraintsAndClose(sysmem_allocator_, std::move(renderer_token),
                               std::move(constraints));
        return fpromise::make_ok_promise();
      });

  ASSERT_TRUE(RunPromise(display_compositor_->ImportBufferCollection(
      kGlobalBufferCollectionId, sysmem_allocator_, std::move(compositor_token),
      BufferCollectionUsage::kClientImage, std::nullopt)));

  {
    auto wait_result = client_collection->WaitForAllBuffersAllocated();
    ASSERT_TRUE(wait_result.is_ok());
    EXPECT_EQ(wait_result->buffer_collection_info().value().buffers().value().size(), 2u);
  }

  // ImportBufferImage() to confirm that the allocation was handled correctly.
  EXPECT_CALL(*renderer_, ImportBufferImage(_, _)).WillOnce(ReturnPromise(fpromise::ok()));
  ASSERT_TRUE(RunPromise(display_compositor_->ImportBufferImage(
      ImageMetadata{.collection_id = kGlobalBufferCollectionId,
                    .identifier = display::ImageId(1),
                    .vmo_index = 0,
                    .width = 1,
                    .height = 1},
      BufferCollectionUsage::kClientImage)));
  EXPECT_FALSE(BufferCollectionSupportsDisplay(kGlobalBufferCollectionId));
}

TEST_F(DisplayCompositorTest, SysmemNegotiationTest_InRendererOnlyMode_DisplayShouldExcludeItself) {
  ForceRendererOnlyMode(true);

  // Create two tokens: one for acting as the "client" and inspecting allocation results with, and
  // one to send to the display compositor.
  auto client_token = CreateToken();
  auto compositor_token = DuplicateToken(client_token);

  // Set "client" constraints.
  auto [client_collection_client_end, client_collection_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator_->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(client_token))
          .buffer_collection_request(std::move(client_collection_server_end))
          .Build());
  ASSERT_TRUE(result.ok());
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> client_collection(
      std::move(client_collection_client_end));

  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  fuchsia_sysmem2::BufferUsage usage;
  usage.cpu(fuchsia_sysmem2::kCpuUsageWrite);
  constraints.usage(std::move(usage));
  fuchsia_sysmem2::BufferMemoryConstraints bmc;
  bmc.min_size_bytes(1);
  bmc.max_size_bytes(20);
  constraints.buffer_memory_constraints(std::move(bmc));
  fuchsia_sysmem2::ImageFormatConstraints ifc;
  ifc.pixel_format(fuchsia_images2::PixelFormat::kB8G8R8A8);
  ifc.color_spaces({{fuchsia_images2::ColorSpace::kSrgb}});
  ifc.min_size(fuchsia_math::SizeU(1, 1));
  constraints.image_format_constraints({{std::move(ifc)}});
  set_constraints_request.constraints(std::move(constraints));
  auto set_constraints_result =
      client_collection->SetConstraints(std::move(set_constraints_request));
  ASSERT_TRUE(set_constraints_result.is_ok());

  const auto kGlobalBufferCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const display::WireBufferCollectionId kDisplayBufferCollectionId =
      display::ToDisplayFidlBufferCollectionId(kGlobalBufferCollectionId);

  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());

  // Set renderer constraints.
  EXPECT_CALL(*renderer_, ImportBufferCollection(kGlobalBufferCollectionId, _, _, _, _))
      .WillOnce([this](auto id, auto& client, auto renderer_token, auto usage, auto size) {
        fuchsia_sysmem2::BufferCollectionConstraints constraints;
        fuchsia_sysmem2::BufferUsage buf_usage;
        buf_usage.cpu(fuchsia_sysmem2::kCpuUsageWrite);
        constraints.usage(std::move(buf_usage));
        constraints.min_buffer_count(2);
        constraints.max_buffer_count(2);
        SetConstraintsAndClose(sysmem_allocator_, std::move(renderer_token),
                               std::move(constraints));
        return fpromise::make_ok_promise();
      });

  // Import BufferCollection and image to trigger constraint setting and handling of allocations.
  ASSERT_TRUE(RunPromise(display_compositor_->ImportBufferCollection(
      kGlobalBufferCollectionId, sysmem_allocator_, std::move(compositor_token),
      BufferCollectionUsage::kClientImage, std::nullopt)));

  {
    auto wait_result = client_collection->WaitForAllBuffersAllocated();
    ASSERT_TRUE(wait_result.is_ok());
    EXPECT_EQ(wait_result->buffer_collection_info().value().buffers().value().size(), 2u);
  }

  // ImportBufferImage() to confirm that the allocation was handled correctly.
  EXPECT_CALL(*renderer_, ImportBufferImage(_, _)).WillOnce(ReturnPromise(fpromise::ok()));
  ASSERT_TRUE(RunPromise(display_compositor_->ImportBufferImage(
      ImageMetadata{.collection_id = kGlobalBufferCollectionId,
                    .identifier = display::ImageId(1),
                    .vmo_index = 0,
                    .width = 1,
                    .height = 1},
      BufferCollectionUsage::kClientImage)));
  EXPECT_FALSE(BufferCollectionSupportsDisplay(kGlobalBufferCollectionId));
}

TEST_F(DisplayCompositorTest, ClientDropSysmemToken) {
  const auto kGlobalBufferCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const display::WireBufferCollectionId kDisplayBufferCollectionId =
      display::ToDisplayFidlBufferCollectionId(kGlobalBufferCollectionId);

  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> dup_token;
  // Let client drop token.
  {
    auto token = CreateToken();
    fidl::SyncClient<fuchsia_sysmem2::BufferCollectionToken> sync_token(std::move(token));
    fuchsia_sysmem2::BufferCollectionTokenDuplicateSyncRequest dup_request;
    dup_request.rights_attenuation_masks({{ZX_RIGHT_SAME_RIGHTS}});
    auto dup_result = sync_token->DuplicateSync(std::move(dup_request));
    ASSERT_TRUE(dup_result.is_ok());
    ASSERT_TRUE(dup_result->tokens().has_value());
    ASSERT_EQ(dup_result->tokens()->size(), 1u);

    dup_token = std::move(dup_result->tokens()->at(0));
  }

  EXPECT_TRUE(RunWithTimeoutOrUntil(
      [&] {
        zx_status_t status = fidl::WireCall(dup_token)->Sync().status();
        return (status != ZX_OK || status == ZX_ERR_PEER_CLOSED);
      },
      zx::duration::infinite(), zx::msec(50)));

  EXPECT_CALL(*renderer_, ImportBufferCollection(kGlobalBufferCollectionId, _, _, _, _)).Times(0);

  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());

  EXPECT_CALL(*mock_display_coordinator_, ImportBufferCollection(_, _)).Times(0);

  EXPECT_FALSE(RunPromise(display_compositor_->ImportBufferCollection(
      kGlobalBufferCollectionId, sysmem_allocator_, std::move(dup_token),
      BufferCollectionUsage::kClientImage, std::nullopt)));
}

TEST_F(DisplayCompositorTest, ImageIsValidAfterReleaseBufferCollection) {
  const auto kGlobalBufferCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const display::WireBufferCollectionId kDisplayBufferCollectionId =
      display::ToDisplayFidlBufferCollectionId(kGlobalBufferCollectionId);

  // Import buffer collection.
  EXPECT_CALL(*mock_display_coordinator_,
              ImportBufferCollection(MatchRequestField(ImportBufferCollection, buffer_collection_id,
                                                       Eq(kDisplayBufferCollectionId)),
                                     _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorImportBufferCollectionRequest* request,
             MockDisplayCoordinator::ImportBufferCollectionCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));
  EXPECT_CALL(*mock_display_coordinator_,
              SetBufferCollectionConstraints(
                  MatchRequestField(SetBufferCollectionConstraints, buffer_collection_id,
                                    Eq(kDisplayBufferCollectionId)),
                  _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorSetBufferCollectionConstraintsRequest*,
             MockDisplayCoordinator::SetBufferCollectionConstraintsCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));
  // Save token to avoid early token failure.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_ref;
  EXPECT_CALL(*renderer_, ImportBufferCollection(kGlobalBufferCollectionId, _, _, _, _))
      .WillOnce([&token_ref](auto id, auto& client, auto token, auto usage, auto size) {
        token_ref = std::move(token);
        return fpromise::make_ok_promise();
      });
  EXPECT_TRUE(RunPromise(display_compositor_->ImportBufferCollection(
      kGlobalBufferCollectionId, sysmem_allocator_, CreateToken(),
      BufferCollectionUsage::kClientImage, std::nullopt)));
  SetDisplaySupported(kGlobalBufferCollectionId, true);

  // Import image.
  ImageMetadata image_metadata = ImageMetadata{
      .collection_id = kGlobalBufferCollectionId,
      .identifier = allocation::GenerateUniqueImageId(),
      .vmo_index = 0,
      .width = 128,
      .height = 256,
  };
  EXPECT_CALL(*mock_display_coordinator_,
              ImportImage(testing::AllOf(MatchRequestField(ImportImage, buffer_collection_id,
                                                           Eq(kDisplayBufferCollectionId)),
                                         MatchRequestField(ImportImage, buffer_index, Eq(0))),
                          _))
      .Times(1)
      .WillOnce(testing::Invoke([](fuchsia_hardware_display::wire::CoordinatorImportImageRequest*,
                                   MockDisplayCoordinator::ImportImageCompleter::Sync& completer) {
        completer.Reply(fit::ok());
      }));
  EXPECT_CALL(*renderer_, ImportBufferImage(image_metadata, _))
      .WillOnce(ReturnPromise(fpromise::ok()));
  EXPECT_TRUE(RunPromise(
      display_compositor_->ImportBufferImage(image_metadata, BufferCollectionUsage::kClientImage)));

  // Release buffer collection. Make sure that does not release Image.
  const display::WireImageId kFidlImageId = image_metadata.identifier.ToFidl();
  EXPECT_CALL(*mock_display_coordinator_,
              ReleaseImage(MatchRequestField(ReleaseImage, image_id, Eq(kFidlImageId)), _))
      .Times(0);
  EXPECT_CALL(
      *mock_display_coordinator_,
      ReleaseBufferCollection(MatchRequestField(ReleaseBufferCollection, buffer_collection_id,
                                                Eq(kDisplayBufferCollectionId)),
                              _))
      .Times(1)
      .WillOnce(Return());
  EXPECT_CALL(*renderer_, ReleaseBufferCollection(kGlobalBufferCollectionId, _)).WillOnce(Return());
  display_compositor_->ReleaseBufferCollection(kGlobalBufferCollectionId,
                                               BufferCollectionUsage::kClientImage);

  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());
}

TEST_F(DisplayCompositorTest, ImportImageErrorCases) {
  const allocation::GlobalBufferCollectionId kGlobalBufferCollectionId =
      allocation::GenerateUniqueBufferCollectionId();
  const display::WireBufferCollectionId kDisplayBufferCollectionId =
      display::ToDisplayFidlBufferCollectionId(kGlobalBufferCollectionId);

  const allocation::GlobalImageId kImageId = allocation::GenerateUniqueImageId();
  const display::WireImageId kFidlImageId = kImageId.ToFidl();
  const uint32_t kVmoCount = 2;
  const uint32_t kVmoIdx = 1;
  const uint32_t kMaxWidth = 100;
  const uint32_t kMaxHeight = 200;
  uint32_t num_times_import_image_called = 0;

  EXPECT_CALL(*mock_display_coordinator_,
              ImportBufferCollection(MatchRequestField(ImportBufferCollection, buffer_collection_id,
                                                       Eq(kDisplayBufferCollectionId)),
                                     _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorImportBufferCollectionRequest* request,
             MockDisplayCoordinator::ImportBufferCollectionCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));
  EXPECT_CALL(*mock_display_coordinator_,
              SetBufferCollectionConstraints(
                  MatchRequestField(SetBufferCollectionConstraints, buffer_collection_id,
                                    Eq(kDisplayBufferCollectionId)),
                  _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorSetBufferCollectionConstraintsRequest*,
             MockDisplayCoordinator::SetBufferCollectionConstraintsCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));
  // Save token to avoid early token failure.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_ref;
  EXPECT_CALL(*renderer_, ImportBufferCollection(kGlobalBufferCollectionId, _, _, _, _))
      .WillOnce([&token_ref](auto id, auto& client, auto token, auto usage, auto size) {
        token_ref = std::move(token);
        return fpromise::make_ok_promise();
      });

  EXPECT_TRUE(RunPromise(display_compositor_->ImportBufferCollection(
      kGlobalBufferCollectionId, sysmem_allocator_, CreateToken(),
      BufferCollectionUsage::kClientImage, std::nullopt)));
  SetDisplaySupported(kGlobalBufferCollectionId, true);

  ImageMetadata metadata = {
      .collection_id = kGlobalBufferCollectionId,
      .identifier = kImageId,
      .vmo_index = kVmoIdx,
      .width = 20,
      .height = 30,
  };

  // Make sure that the engine returns true if the display coordinator returns true.
  EXPECT_CALL(*mock_display_coordinator_,
              ImportImage(testing::AllOf(MatchRequestField(ImportImage, buffer_collection_id,
                                                           Eq(kDisplayBufferCollectionId)),
                                         MatchRequestField(ImportImage, buffer_index, Eq(kVmoIdx))),
                          _))
      .Times(1)
      .WillOnce(testing::Invoke([](fuchsia_hardware_display::wire::CoordinatorImportImageRequest*,
                                   MockDisplayCoordinator::ImportImageCompleter::Sync& completer) {
        completer.Reply(fit::ok());
      }));

  EXPECT_CALL(*renderer_, ImportBufferImage(metadata, _)).WillOnce(ReturnPromise(fpromise::ok()));

  EXPECT_TRUE(RunPromise(
      display_compositor_->ImportBufferImage(metadata, BufferCollectionUsage::kClientImage)));

  // Make sure we can release the image properly.
  EXPECT_CALL(*mock_display_coordinator_,
              ReleaseImage(MatchRequestField(ReleaseImage, image_id, Eq(kFidlImageId)), _))
      .Times(1)
      .WillOnce(Return());
  EXPECT_CALL(*renderer_, ReleaseBufferImage(metadata.identifier)).WillOnce(Return());

  display_compositor_->ReleaseBufferImage(metadata.identifier);

  // Make sure that the engine returns false if the display coordinator returns an error
  EXPECT_CALL(*mock_display_coordinator_,
              ImportImage(testing::AllOf(MatchRequestField(ImportImage, buffer_collection_id,
                                                           Eq(kDisplayBufferCollectionId)),
                                         MatchRequestField(ImportImage, buffer_index, Eq(kVmoIdx))),
                          _))
      .Times(1)
      .WillOnce(testing::Invoke([](fuchsia_hardware_display::wire::CoordinatorImportImageRequest*,
                                   MockDisplayCoordinator::ImportImageCompleter::Sync& completer) {
        completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
      }));

  // This should still return false for the engine even if the renderer returns true.
  EXPECT_CALL(*renderer_, ImportBufferImage(metadata, _)).WillOnce(ReturnPromise(fpromise::ok()));

  EXPECT_FALSE(RunPromise(
      display_compositor_->ImportBufferImage(metadata, BufferCollectionUsage::kClientImage)));

  // Collection ID can't be invalid. This shouldn't reach the display coordinator.
  EXPECT_CALL(*mock_display_coordinator_,
              ImportImage(testing::AllOf(MatchRequestField(ImportImage, buffer_collection_id,
                                                           Eq(kDisplayBufferCollectionId)),
                                         MatchRequestField(ImportImage, buffer_index, Eq(kVmoIdx))),
                          _))
      .Times(0);
  auto copy_metadata = metadata;
  copy_metadata.collection_id = allocation::kInvalidId;
  EXPECT_FALSE(RunPromise(
      display_compositor_->ImportBufferImage(copy_metadata, BufferCollectionUsage::kClientImage)));

  // Image Id can't be 0. This shouldn't reach the display coordinator.
  EXPECT_CALL(*mock_display_coordinator_,
              ImportImage(testing::AllOf(MatchRequestField(ImportImage, buffer_collection_id,
                                                           Eq(kDisplayBufferCollectionId)),
                                         MatchRequestField(ImportImage, buffer_index, Eq(kVmoIdx))),
                          _))
      .Times(0);
  copy_metadata = metadata;
  copy_metadata.identifier = display::kInvalidImageId;
  EXPECT_FALSE(RunPromise(
      display_compositor_->ImportBufferImage(copy_metadata, BufferCollectionUsage::kClientImage)));

  // Width can't be 0. This shouldn't reach the display coordinator.
  EXPECT_CALL(*mock_display_coordinator_,
              ImportImage(testing::AllOf(MatchRequestField(ImportImage, buffer_collection_id,
                                                           Eq(kDisplayBufferCollectionId)),
                                         MatchRequestField(ImportImage, buffer_index, Eq(kVmoIdx))),
                          _))
      .Times(0);
  copy_metadata = metadata;
  copy_metadata.width = 0;
  EXPECT_FALSE(RunPromise(
      display_compositor_->ImportBufferImage(copy_metadata, BufferCollectionUsage::kClientImage)));

  // Height can't be 0. This shouldn't reach the display coordinator.
  EXPECT_CALL(*mock_display_coordinator_,
              ImportImage(testing::AllOf(MatchRequestField(ImportImage, buffer_collection_id,
                                                           Eq(kDisplayBufferCollectionId)),
                                         MatchRequestField(ImportImage, buffer_index, Eq(kVmoIdx))),
                          _))
      .Times(0);
  copy_metadata = metadata;
  copy_metadata.height = 0;
  EXPECT_FALSE(RunPromise(
      display_compositor_->ImportBufferImage(copy_metadata, BufferCollectionUsage::kClientImage)));

  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());
}

// This test checks that DisplayCompositor properly processes ConfigStamp from Vsync.
TEST_F(DisplayCompositorTest, VsyncConfigStampAreProcessed) {
  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());
  EXPECT_CALL(*mock_display_coordinator_, CheckConfig(_))
      .Times(1)
      .WillRepeatedly(
          testing::Invoke([&](MockDisplayCoordinator::CheckConfigCompleter::Sync& completer) {
            completer.Reply(display::WireConfigResult::kOk);
          }));

  static constexpr display::WireConfigStamp kConfigStamp1(1);
  static constexpr display::WireConfigStamp kConfigStamp2(2);
  EXPECT_CALL(*mock_display_coordinator_, CommitConfig(_, _)).Times(2).WillRepeatedly(Return());
  display_compositor_->RenderFrame(1, zx::time_monotonic(1), std::span<const RenderData>(), {}, {},
                                   {}, [](const scheduling::Timestamps&) {});
  display_compositor_->RenderFrame(2, zx::time_monotonic(2), std::span<const RenderData>(), {}, {},
                                   {}, [](const scheduling::Timestamps&) {});

  EXPECT_EQ(2u, GetPendingApplyConfigs().size());

  // Sending another vsync should be skipped.
  static constexpr display::WireConfigStamp kConfigStamp3(3);
  SendOnVsyncEvent({kConfigStamp3});
  EXPECT_EQ(2u, GetPendingApplyConfigs().size());

  // Sending later vsync should signal and remove the earlier one too.
  SendOnVsyncEvent(kConfigStamp2);
  EXPECT_EQ(0u, GetPendingApplyConfigs().size());
}

// When compositing directly to a hardware display layer, the display coordinator
// takes in source and destination Frame object types, which mirrors flatland usage.
// The source frames are nonnormalized UV coordinates and the destination frames are
// screenspace coordinates given in pixels. So this test makes sure that the rectangle
// and frame data that is generated by flatland sends along to the display coordinator
// the proper source and destination frame data. Each source and destination frame pair
// should be added to its own layer on the display.
TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessTest) {
  const uint64_t kGlobalBufferCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const display::WireBufferCollectionId kDisplayBufferCollectionId =
      display::ToDisplayFidlBufferCollectionId(kGlobalBufferCollectionId);

  // Add an image.
  ImageMetadata parent_image_metadata = ImageMetadata{
      .collection_id = kGlobalBufferCollectionId,
      .identifier = allocation::GenerateUniqueImageId(),
      .vmo_index = 0,
      .width = 128,
      .height = 256,
  };

  // Add an image.
  ImageMetadata child_image_metadata = ImageMetadata{
      .collection_id = kGlobalBufferCollectionId,
      .identifier = allocation::GenerateUniqueImageId(),
      .vmo_index = 1,
      .width = 512,
      .height = 1024,
  };

  const display::DisplayId kDisplayId(1);
  glm::uvec2 resolution(1024, 768);

  // We will end up with 2 source frames, 2 destination frames, and two layers being sent to the
  // display.
  const fuchsia_math::wire::RectU kExpectedSources[2] = {
      {.x = 0u, .y = 0u, .width = 512, .height = 1024u},
      {.x = 0u, .y = 0u, .width = 128u, .height = 256u},
  };

  const fuchsia_math::wire::RectU kExpectedDestinations[2] = {
      {.x = 5u, .y = 7u, .width = 30, .height = 40u},
      {.x = 9u, .y = 13u, .width = 10u, .height = 20u},
  };

  auto MakeRect = [](const fuchsia_math::wire::RectU& src, const fuchsia_math::wire::RectU& dst) {
    return SrcToDest(types::RectangleF({.x = static_cast<float>(src.x),
                                        .y = static_cast<float>(src.y),
                                        .width = static_cast<float>(src.width),
                                        .height = static_cast<float>(src.height)}),
                     types::RectangleF({.x = static_cast<float>(dst.x),
                                        .y = static_cast<float>(dst.y),
                                        .width = static_cast<float>(dst.width),
                                        .height = static_cast<float>(dst.height)}),
                     types::RotateFlip::kIdentity());
  };

  EXPECT_CALL(*mock_display_coordinator_,
              ImportBufferCollection(MatchRequestField(ImportBufferCollection, buffer_collection_id,
                                                       Eq(kDisplayBufferCollectionId)),
                                     _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorImportBufferCollectionRequest* request,
             MockDisplayCoordinator::ImportBufferCollectionCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));
  EXPECT_CALL(*mock_display_coordinator_,
              SetBufferCollectionConstraints(
                  MatchRequestField(SetBufferCollectionConstraints, buffer_collection_id,
                                    Eq(kDisplayBufferCollectionId)),
                  _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorSetBufferCollectionConstraintsRequest*,
             MockDisplayCoordinator::SetBufferCollectionConstraintsCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));
  // Save token to avoid early token failure.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_ref;
  EXPECT_CALL(*renderer_, ImportBufferCollection(kGlobalBufferCollectionId, _, _, _, _))
      .WillOnce([&token_ref](auto id, auto& client, auto token, auto usage, auto size) {
        token_ref = std::move(token);
        return fpromise::make_ok_promise();
      });
  EXPECT_TRUE(RunPromise(display_compositor_->ImportBufferCollection(
      kGlobalBufferCollectionId, sysmem_allocator_, CreateToken(),
      BufferCollectionUsage::kClientImage, std::nullopt)));
  SetDisplaySupported(kGlobalBufferCollectionId, true);

  const display::WireImageId fidl_parent_image_id = parent_image_metadata.identifier.ToFidl();
  EXPECT_CALL(*mock_display_coordinator_,
              ImportImage(testing::AllOf(
                              MatchRequestField(ImportImage, buffer_collection_id,
                                                Eq(kDisplayBufferCollectionId)),
                              MatchRequestField(ImportImage, buffer_index, Eq(0)),
                              MatchRequestField(ImportImage, image_id, Eq(fidl_parent_image_id))),
                          _))
      .Times(1)
      .WillOnce(testing::Invoke([](fuchsia_hardware_display::wire::CoordinatorImportImageRequest*,
                                   MockDisplayCoordinator::ImportImageCompleter::Sync& completer) {
        completer.Reply(fit::ok());
      }));
  EXPECT_CALL(*renderer_, ImportBufferImage(parent_image_metadata, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  EXPECT_TRUE(RunPromise(display_compositor_->ImportBufferImage(
      parent_image_metadata, BufferCollectionUsage::kClientImage)));

  const display::WireImageId fidl_child_image_id = child_image_metadata.identifier.ToFidl();

  EXPECT_CALL(
      *mock_display_coordinator_,
      ImportImage(testing::AllOf(MatchRequestField(ImportImage, buffer_collection_id,
                                                   Eq(kDisplayBufferCollectionId)),
                                 MatchRequestField(ImportImage, buffer_index, Eq(1)),
                                 MatchRequestField(ImportImage, image_id, Eq(fidl_child_image_id))),
                  _))
      .Times(1)
      .WillOnce(testing::Invoke([](fuchsia_hardware_display::wire::CoordinatorImportImageRequest*,
                                   MockDisplayCoordinator::ImportImageCompleter::Sync& completer) {
        completer.Reply(fit::ok());
      }));

  EXPECT_CALL(*renderer_, ImportBufferImage(child_image_metadata, _))
      .WillOnce(ReturnPromise(fpromise::ok()));
  EXPECT_TRUE(RunPromise(display_compositor_->ImportBufferImage(
      child_image_metadata, BufferCollectionUsage::kClientImage)));

  EXPECT_CALL(*renderer_, SetColorConversionValues(_, _, _)).WillOnce(Return());
  display_compositor_->SetColorConversionValues({1, 0, 0, 0, 1, 0, 0, 0, 1}, {0.1f, 0.2f, 0.3f},
                                                {-0.3f, -0.2f, -0.1f});

  // Setup the EXPECT_CALLs for gmock.
  uint64_t layer_id_value = 1;
  EXPECT_CALL(*mock_display_coordinator_, CreateLayer(_, _))
      .Times(3)
      .WillRepeatedly(testing::Invoke(
          [&](fidl::WireServer<fuchsia_hardware_display::Coordinator>::CreateLayerRequestView
                  request,
              MockDisplayCoordinator::CreateLayerCompleter::Sync& completer) {
            EXPECT_EQ(request->layer_id.value, layer_id_value++);
            completer.Reply(fit::ok());
          }));
  EXPECT_CALL(*mock_display_coordinator_, SetLayerColorConfig(_, _)).Times(1).WillOnce(Return());

  std::vector<display::WireLayerId> layers = {{.value = 2}, {.value = 3}};

  EXPECT_CALL(
      *mock_display_coordinator_,
      SetDisplayLayers(
          testing::AllOf(
              MatchRequestField(SetDisplayLayers, display_id, Eq(kDisplayId.ToFidl())),
              MatchRequestField(SetDisplayLayers, layer_ids, testing::ElementsAreArray(layers))),
          _))
      .Times(1)
      .WillOnce(Return());

  // Make sure each layer has all of its components set properly.
  display::WireImageId fidl_image_ids[] = {child_image_metadata.identifier.ToFidl(),
                                           parent_image_metadata.identifier.ToFidl()};
  for (uint32_t i = 0; i < 2; i++) {
    EXPECT_CALL(
        *mock_display_coordinator_,
        SetLayerPrimaryConfig(MatchRequestField(SetLayerPrimaryConfig, layer_id, Eq(layers[i])), _))
        .Times(1)
        .WillOnce(Return());

    EXPECT_CALL(
        *mock_display_coordinator_,
        SetLayerPrimaryPosition(
            testing::AllOf(MatchRequestField(SetLayerPrimaryPosition, layer_id, Eq(layers[i])),
                           MatchRequestField(SetLayerPrimaryPosition, image_source_transformation,
                                             Eq(display::WireCoordinateTransformation::kIdentity))),
            _))
        .Times(1)
        .WillOnce(testing::Invoke(
            [kExpectedSources, kExpectedDestinations, index = i](
                fuchsia_hardware_display::wire::CoordinatorSetLayerPrimaryPositionRequest* request,
                MockDisplayCoordinator::SetLayerPrimaryPositionCompleter::Sync& completer) {
              EXPECT_EQ(request->image_source, kExpectedSources[index]);
              EXPECT_EQ(request->display_destination, kExpectedDestinations[index]);
            }));

    EXPECT_CALL(
        *mock_display_coordinator_,
        SetLayerPrimaryAlpha(MatchRequestField(SetLayerPrimaryAlpha, layer_id, Eq(layers[i])), _))
        .Times(1)
        .WillOnce(Return());

    EXPECT_CALL(
        *mock_display_coordinator_,
        SetLayerImage2(
            testing::AllOf(MatchRequestField(SetLayerImage2, layer_id, Eq(layers[i])),
                           MatchRequestField(SetLayerImage2, image_id, Eq(fidl_image_ids[i]))),
            _))
        .Times(1)
        .WillOnce(Return());
  }

  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1);
  EXPECT_CALL(*mock_display_coordinator_, SetDisplayColorConversion(_, _)).Times(1);
  EXPECT_CALL(*mock_display_coordinator_, SetDisplayMode(_, _)).Times(1);

  EXPECT_CALL(*mock_display_coordinator_, CheckConfig(_))
      .Times(1)
      .WillOnce(testing::Invoke([&](MockDisplayCoordinator::CheckConfigCompleter::Sync& completer) {
        completer.Reply(display::WireConfigResult::kOk);
      }));

  EXPECT_CALL(*renderer_, ChoosePreferredRenderTargetFormat(_));

  DisplayInfo display_info = {resolution, {kPixelFormat}, kMaxDisplayLayersCount};
  display::Display display({kDisplayId.ToFidl()}, resolution.x, resolution.y,
                           kMaxDisplayLayersCount);
  display_compositor_->AddDisplay(&display, display_info, /*num_vmos*/ 0,
                                  /*out_buffer_collection*/ nullptr);

  EXPECT_CALL(*mock_display_coordinator_, CommitConfig(_, _)).Times(1).WillOnce(Return());

  ResolvedLayer child_layer = {
      .geometry = MakeRect(kExpectedSources[0], kExpectedDestinations[0]),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::ImageContent{.image_id = child_image_metadata.identifier,
                                             .width = child_image_metadata.width,
                                             .height = child_image_metadata.height},
  };
  ResolvedLayer parent_layer = {
      .geometry = MakeRect(kExpectedSources[1], kExpectedDestinations[1]),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::ImageContent{.image_id = parent_image_metadata.identifier,
                                             .width = parent_image_metadata.width,
                                             .height = parent_image_metadata.height},
  };
  std::array resolved_layers = {child_layer, parent_layer};
  RenderData render_data = {.display_id = kDisplayId, .layers = resolved_layers};
  std::span<const RenderData> render_data_list(&render_data, 1);

  display_compositor_->RenderFrame(1, zx::time_monotonic(1), render_data_list, {}, {}, {},
                                   [](const scheduling::Timestamps&) {});

  // Cleanup: All layers should be destroyed.
  for (uint64_t i = 1; i < layer_id_value; ++i) {
    EXPECT_CALL(
        *mock_display_coordinator_,
        DestroyLayer(
            MatchRequestField(DestroyLayer, layer_id, Eq(display::WireLayerId{.value = i})), _))
        .Times(1)
        .WillOnce(Return());
  }
}

// The rotation and flip tests below share the following setup:
// the layer draws a 128×256 image (full sample region) into a 10×20 destination rect
// at the origin; each test below varies only the rotation/flip.
void DisplayCompositorTest::HardwareFrameCorrectnessWithRotationTester(
    Orientation orientation, ImageFlip image_flip, const fuchsia_math::wire::RectU expected_dst,
    display::WireCoordinateTransformation expected_transform) {
  const uint64_t kGlobalBufferCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const display::WireBufferCollectionId kDisplayBufferCollectionId =
      display::ToDisplayFidlBufferCollectionId(kGlobalBufferCollectionId);

  // Add an image.
  ImageMetadata parent_image_metadata = ImageMetadata{
      .collection_id = kGlobalBufferCollectionId,
      .identifier = allocation::GenerateUniqueImageId(),
      .vmo_index = 0,
      .width = 128,
      .height = 256,
  };

  const display::DisplayId kDisplayId(1);
  glm::uvec2 resolution(1024, 768);

  // We will end up with 1 source frame, 1 destination frame, and one layer being sent to the
  // display.
  const fuchsia_math::wire::RectU kExpectedSource = {
      .x = 0u, .y = 0u, .width = 128u, .height = 256u};

  EXPECT_CALL(*mock_display_coordinator_,
              ImportBufferCollection(MatchRequestField(ImportBufferCollection, buffer_collection_id,
                                                       Eq(kDisplayBufferCollectionId)),
                                     _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorImportBufferCollectionRequest* request,
             MockDisplayCoordinator::ImportBufferCollectionCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));
  EXPECT_CALL(*mock_display_coordinator_,
              SetBufferCollectionConstraints(
                  MatchRequestField(SetBufferCollectionConstraints, buffer_collection_id,
                                    Eq(kDisplayBufferCollectionId)),
                  _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorSetBufferCollectionConstraintsRequest*,
             MockDisplayCoordinator::SetBufferCollectionConstraintsCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));
  // Save token to avoid early token failure.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_ref;
  EXPECT_CALL(*renderer_, ImportBufferCollection(kGlobalBufferCollectionId, _, _, _, _))
      .WillOnce([&token_ref](auto id, auto& client, auto token, auto usage, auto size) {
        token_ref = std::move(token);
        return fpromise::make_ok_promise();
      });
  EXPECT_TRUE(RunPromise(display_compositor_->ImportBufferCollection(
      kGlobalBufferCollectionId, sysmem_allocator_, CreateToken(),
      BufferCollectionUsage::kClientImage, std::nullopt)));
  SetDisplaySupported(kGlobalBufferCollectionId, true);

  const display::WireImageId fidl_parent_image_id = parent_image_metadata.identifier.ToFidl();
  EXPECT_CALL(*mock_display_coordinator_,
              ImportImage(testing::AllOf(
                              MatchRequestField(ImportImage, buffer_collection_id,
                                                Eq(kDisplayBufferCollectionId)),
                              MatchRequestField(ImportImage, buffer_index, Eq(0)),
                              MatchRequestField(ImportImage, image_id, Eq(fidl_parent_image_id))),
                          _))
      .Times(1)
      .WillOnce(testing::Invoke([](fuchsia_hardware_display::wire::CoordinatorImportImageRequest*,
                                   MockDisplayCoordinator::ImportImageCompleter::Sync& completer) {
        completer.Reply(fit::ok());
      }));

  EXPECT_CALL(*renderer_, ImportBufferImage(parent_image_metadata, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  EXPECT_TRUE(RunPromise(display_compositor_->ImportBufferImage(
      parent_image_metadata, BufferCollectionUsage::kClientImage)));

  EXPECT_CALL(*renderer_, SetColorConversionValues(_, _, _)).WillOnce(Return());

  display_compositor_->SetColorConversionValues({1, 0, 0, 0, 1, 0, 0, 0, 1}, {0.1f, 0.2f, 0.3f},
                                                {-0.3f, -0.2f, -0.1f});

  // Setup the EXPECT_CALLs for gmock.
  // Note that a couple of layers are created upfront for the display.
  uint64_t layer_id_value = 1;
  EXPECT_CALL(*mock_display_coordinator_, CreateLayer(_, _))
      .Times(3)
      .WillRepeatedly(testing::Invoke(
          [&](fidl::WireServer<fuchsia_hardware_display::Coordinator>::CreateLayerRequestView
                  request,
              MockDisplayCoordinator::CreateLayerCompleter::Sync& completer) {
            EXPECT_EQ(request->layer_id.value, layer_id_value++);
            completer.Reply(fit::ok());
          }));
  EXPECT_CALL(*mock_display_coordinator_, SetLayerColorConfig(_, _)).Times(1).WillOnce(Return());

  // However, we only set one display layer for the image.
  const std::vector<fuchsia_hardware_display::wire::LayerId> layers = {{.value = 2}};
  EXPECT_CALL(
      *mock_display_coordinator_,
      SetDisplayLayers(
          testing::AllOf(
              MatchRequestField(SetDisplayLayers, display_id, Eq(kDisplayId.ToFidl())),
              MatchRequestField(SetDisplayLayers, layer_ids, testing::ElementsAreArray(layers))),
          _))
      .Times(1)
      .WillOnce(Return());

  const display::WireImageId fidl_collection_image_id = parent_image_metadata.identifier.ToFidl();
  EXPECT_CALL(
      *mock_display_coordinator_,
      SetLayerPrimaryConfig(MatchRequestField(SetLayerPrimaryConfig, layer_id, Eq(layers[0])), _))
      .Times(1)
      .WillOnce(Return());
  EXPECT_CALL(
      *mock_display_coordinator_,
      SetLayerPrimaryPosition(
          testing::AllOf(MatchRequestField(SetLayerPrimaryPosition, layer_id, Eq(layers[0])),
                         MatchRequestField(SetLayerPrimaryPosition, image_source_transformation,
                                           Eq(expected_transform))),
          _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [kExpectedSource, expected_dst](
              fuchsia_hardware_display::wire::CoordinatorSetLayerPrimaryPositionRequest* request,
              MockDisplayCoordinator::SetLayerPrimaryPositionCompleter::Sync& completer) {
            EXPECT_EQ(request->image_source, kExpectedSource);
            EXPECT_EQ(request->display_destination, expected_dst);
          }));
  EXPECT_CALL(
      *mock_display_coordinator_,
      SetLayerPrimaryAlpha(MatchRequestField(SetLayerPrimaryAlpha, layer_id, Eq(layers[0])), _))
      .Times(1)
      .WillOnce(Return());
  EXPECT_CALL(
      *mock_display_coordinator_,
      SetLayerImage2(
          testing::AllOf(MatchRequestField(SetLayerImage2, layer_id, Eq(layers[0])),
                         MatchRequestField(SetLayerImage2, image_id, Eq(fidl_collection_image_id))),
          _))
      .Times(1)
      .WillOnce(Return());

  EXPECT_CALL(*mock_display_coordinator_, SetDisplayColorConversion(_, _)).Times(1);
  EXPECT_CALL(*mock_display_coordinator_, SetDisplayMode(_, _)).Times(1);

  EXPECT_CALL(*mock_display_coordinator_, CheckConfig(_))
      .Times(1)
      .WillOnce(testing::Invoke([&](MockDisplayCoordinator::CheckConfigCompleter::Sync& completer) {
        completer.Reply(display::WireConfigResult::kOk);
      }));

  EXPECT_CALL(*renderer_, ChoosePreferredRenderTargetFormat(_));

  DisplayInfo display_info = {resolution, {kPixelFormat}, kMaxDisplayLayersCount};
  display::Display display({kDisplayId.ToFidl()}, resolution.x, resolution.y,
                           kMaxDisplayLayersCount);
  display_compositor_->AddDisplay(&display, display_info, /*num_vmos*/ 0,
                                  /*out_buffer_collection*/ nullptr);

  EXPECT_CALL(*mock_display_coordinator_, CommitConfig(_, _)).Times(1).WillOnce(Return());

  ResolvedLayer layer = {
      .geometry =
          SrcToDest(types::RectangleF({.x = 0.f, .y = 0.f, .width = 128.f, .height = 256.f}),
                    types::RectangleF({.x = static_cast<float>(expected_dst.x),
                                       .y = static_cast<float>(expected_dst.y),
                                       .width = static_cast<float>(expected_dst.width),
                                       .height = static_cast<float>(expected_dst.height)}),
                    types::RotateFlip::From(expected_transform)),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::ImageContent{.image_id = parent_image_metadata.identifier,
                                             .width = parent_image_metadata.width,
                                             .height = parent_image_metadata.height},
  };
  RenderData render_data = {.display_id = kDisplayId,
                            .layers = std::span<const ResolvedLayer>(&layer, 1)};
  std::span<const RenderData> render_data_list(&render_data, 1);

  display_compositor_->RenderFrame(1, zx::time_monotonic(1), render_data_list, {}, {}, {},
                                   [](const scheduling::Timestamps&) {});

  for (uint64_t i = 1; i < layer_id_value; ++i) {
    EXPECT_CALL(
        *mock_display_coordinator_,
        DestroyLayer(
            MatchRequestField(DestroyLayer, layer_id, Eq(display::WireLayerId{.value = i})), _))
        .Times(1)
        .WillOnce(Return());
  }

  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1);
}

// With90DegreeRotationTest
//   90° CCW rotation swaps the destination's W/H: 10×20 -> 20×10 at the origin.
//   Display transform: rotate CCW 90°.
TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessWith90DegreeRotationTest) {
  const fuchsia_math::wire::RectU kExpectedDest = {.x = 0u, .y = 0u, .width = 20u, .height = 10u};
  HardwareFrameCorrectnessWithRotationTester(Orientation::kCcw90Degrees, ImageFlip::kNone,
                                             kExpectedDest,
                                             display::WireCoordinateTransformation::kRotateCcw90);
}

// With180DegreeRotationTest
//   180° rotation preserves W/H: destination stays 10×20 at the origin.
//   Display transform: rotate CCW 180°.
TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessWith180DegreeRotationTest) {
  const fuchsia_math::wire::RectU kExpectedDest = {.x = 0u, .y = 0u, .width = 10u, .height = 20u};
  HardwareFrameCorrectnessWithRotationTester(Orientation::kCcw180Degrees, ImageFlip::kNone,
                                             kExpectedDest,
                                             display::WireCoordinateTransformation::kRotateCcw180);
}

// With270DegreeRotationTest
//   270° CCW rotation swaps W/H: 10×20 -> 20×10 at the origin.
//   Display transform: rotate CCW 270°.
TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessWith270DegreeRotationTest) {
  const fuchsia_math::wire::RectU kExpectedDest = {.x = 0u, .y = 0u, .width = 20u, .height = 10u};
  HardwareFrameCorrectnessWithRotationTester(Orientation::kCcw270Degrees, ImageFlip::kNone,
                                             kExpectedDest,
                                             display::WireCoordinateTransformation::kRotateCcw270);
}

// WithLeftRightFlipTest
//   No rotation: destination stays 10×20.  A left-right (horizontal) mirror is a
//   reflection across the Y axis (kReflectY); the flip rides ImageFlip.
TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessWithLeftRightFlipTest) {
  const fuchsia_math::wire::RectU kExpectedDest = {.x = 0u, .y = 0u, .width = 10u, .height = 20u};
  HardwareFrameCorrectnessWithRotationTester(Orientation::kCcw0Degrees, ImageFlip::kLeftRight,
                                             kExpectedDest,
                                             display::WireCoordinateTransformation::kReflectY);
}

// WithUpDownFlipTest
//   No rotation: destination stays 10×20.  An up-down (vertical) mirror reflects
//   across the X axis (kReflectX); the flip rides ImageFlip.
TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessWithUpDownFlipTest) {
  const fuchsia_math::wire::RectU kExpectedDest = {.x = 0u, .y = 0u, .width = 10u, .height = 20u};
  HardwareFrameCorrectnessWithRotationTester(Orientation::kCcw0Degrees, ImageFlip::kUpDown,
                                             kExpectedDest,
                                             display::WireCoordinateTransformation::kReflectX);
}

// WithLeftRightFlip90DegreeRotationTest
//   Left-right flip combined with 90° CCW rotation; destination 10×20 -> 20×10.
//   Display transform ROTATE_CCW_90_REFLECT_X: "ROTATE_CCW_90, followed by
//   REFLECT_X" (coordinator.fidl / fuchsia.hardware.display.types.CoordinateTransformation :
//   the combined enums rotate first, then reflect).  Note the reflection is
//   REFLECT_X here, whereas a left-right flip *without* rotation is REFLECT_Y
//   (kReflectY): the display reflects in the post-rotation frame.
TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessWithLeftRightFlip90DegreeRotationTest) {
  const fuchsia_math::wire::RectU kExpectedDest = {.x = 0u, .y = 0u, .width = 20u, .height = 10u};
  HardwareFrameCorrectnessWithRotationTester(
      Orientation::kCcw90Degrees, ImageFlip::kLeftRight, kExpectedDest,
      display::WireCoordinateTransformation::kRotateCcw90ReflectX);
}

// WithUpDownFlip90DegreeRotationTest
//   Up-down flip combined with 90° CCW rotation; destination 10×20 -> 20×10.
//   Display transform ROTATE_CCW_90_REFLECT_Y: "ROTATE_CCW_90, followed by
//   REFLECT_Y" (same rotate-then-reflect order, coordinator.fidl).
TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessWithUpDownFlip90DegreeRotationTest) {
  const fuchsia_math::wire::RectU kExpectedDest = {.x = 0u, .y = 0u, .width = 20u, .height = 10u};
  HardwareFrameCorrectnessWithRotationTester(
      Orientation::kCcw90Degrees, ImageFlip::kUpDown, kExpectedDest,
      display::WireCoordinateTransformation::kRotateCcw90ReflectY);
}

TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessWithLeftRightFlip180DegreeRotationTest) {
  const fuchsia_math::wire::RectU kExpectedDest = {.x = 0u, .y = 0u, .width = 10u, .height = 20u};
  HardwareFrameCorrectnessWithRotationTester(Orientation::kCcw180Degrees, ImageFlip::kLeftRight,
                                             kExpectedDest,
                                             display::WireCoordinateTransformation::kReflectX);
}

TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessWithUpDownFlip180DegreeRotationTest) {
  const fuchsia_math::wire::RectU kExpectedDest = {.x = 0u, .y = 0u, .width = 10u, .height = 20u};
  HardwareFrameCorrectnessWithRotationTester(Orientation::kCcw180Degrees, ImageFlip::kUpDown,
                                             kExpectedDest,
                                             display::WireCoordinateTransformation::kReflectY);
}

TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessWithLeftRightFlip270DegreeRotationTest) {
  const fuchsia_math::wire::RectU kExpectedDest = {.x = 0u, .y = 0u, .width = 20u, .height = 10u};
  HardwareFrameCorrectnessWithRotationTester(
      Orientation::kCcw270Degrees, ImageFlip::kLeftRight, kExpectedDest,
      display::WireCoordinateTransformation::kRotateCcw90ReflectY);
}

TEST_F(DisplayCompositorTest, HardwareFrameCorrectnessWithUpDownFlip270DegreeRotationTest) {
  const fuchsia_math::wire::RectU kExpectedDest = {.x = 0u, .y = 0u, .width = 20u, .height = 10u};
  HardwareFrameCorrectnessWithRotationTester(
      Orientation::kCcw270Degrees, ImageFlip::kUpDown, kExpectedDest,
      display::WireCoordinateTransformation::kRotateCcw90ReflectX);
}

// Tests that RenderOnly mode does not attempt to ImportBufferCollection() to display.
TEST_F(DisplayCompositorTest, RendererOnly_ImportAndReleaseBufferCollectionTest) {
  ForceRendererOnlyMode(true);

  const allocation::GlobalBufferCollectionId kGlobalBufferCollectionId = 15;
  const display::WireBufferCollectionId kDisplayBufferCollectionId =
      display::ToDisplayFidlBufferCollectionId(kGlobalBufferCollectionId);

  EXPECT_CALL(*mock_display_coordinator_,
              ImportBufferCollection(MatchRequestField(ImportBufferCollection, buffer_collection_id,
                                                       Eq(kDisplayBufferCollectionId)),
                                     _))
      .Times(0);
  // Save token to avoid early token failure.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_ref;
  EXPECT_CALL(*renderer_, ImportBufferCollection(kGlobalBufferCollectionId, _, _, _, _))
      .WillOnce([&token_ref](auto id, auto& client, auto token, auto usage, auto size) {
        token_ref = std::move(token);
        return fpromise::make_ok_promise();
      });
  EXPECT_TRUE(RunPromise(display_compositor_->ImportBufferCollection(
      kGlobalBufferCollectionId, sysmem_allocator_, CreateToken(),
      BufferCollectionUsage::kClientImage, std::nullopt)));

  EXPECT_CALL(
      *mock_display_coordinator_,
      ReleaseBufferCollection(MatchRequestField(ReleaseBufferCollection, buffer_collection_id,
                                                Eq(kDisplayBufferCollectionId)),
                              _))
      .Times(1)
      .WillOnce(Return());

  EXPECT_CALL(*renderer_, ReleaseBufferCollection(kGlobalBufferCollectionId, _)).WillOnce(Return());
  display_compositor_->ReleaseBufferCollection(kGlobalBufferCollectionId,
                                               BufferCollectionUsage::kClientImage);

  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());
}

TEST_F(DisplayCompositorTest, SetDisplayLayers_WithNoImages_UsesEmptySceneLayer) {
  const display::DisplayId kDisplayId(1);
  glm::uvec2 resolution(1024, 768);
  DisplayInfo display_info = {resolution, {kPixelFormat}, kMaxDisplayLayersCount};

  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());
  EXPECT_CALL(*mock_display_coordinator_, SetDisplayMode(_, _)).Times(testing::AnyNumber());
  EXPECT_CALL(*mock_display_coordinator_, CheckConfig(_))
      .Times(1)
      .WillRepeatedly(
          testing::Invoke([&](MockDisplayCoordinator::CheckConfigCompleter::Sync& completer) {
            completer.Reply(display::WireConfigResult::kOk);
          }));

  // Setup the EXPECT_CALLs for gmock.
  // We expect 1 layer for empty scene, and 2 layers for the pool (configured in SetUp).
  uint64_t layer_id_value = 1;
  EXPECT_CALL(*mock_display_coordinator_, CreateLayer(_, _))
      .Times(3)
      .WillRepeatedly(testing::Invoke(
          [&](fidl::WireServer<fuchsia_hardware_display::Coordinator>::CreateLayerRequestView
                  request,
              MockDisplayCoordinator::CreateLayerCompleter::Sync& completer) {
            EXPECT_EQ(request->layer_id.value, layer_id_value++);
            completer.Reply(fit::ok());
          }));

  EXPECT_CALL(*mock_display_coordinator_, SetLayerColorConfig(_, _)).Times(1).WillOnce(Return());

  display::Display display({kDisplayId.ToFidl()}, resolution.x, resolution.y,
                           kMaxDisplayLayersCount);
  display_compositor_->AddDisplay(&display, display_info, /*num_vmos*/ 0,
                                  /*out_buffer_collection*/ nullptr);

  // We expect SetDisplayLayers to be called with the FIRST layer created (empty scene layer).
  std::vector<display::WireLayerId> expected_layers = {{.value = 1}};
  EXPECT_CALL(
      *mock_display_coordinator_,
      SetDisplayLayers(
          testing::AllOf(MatchRequestField(SetDisplayLayers, display_id, Eq(kDisplayId.ToFidl())),
                         MatchRequestField(SetDisplayLayers, layer_ids,
                                           testing::ElementsAreArray(expected_layers))),
          _))
      .Times(1)
      .WillOnce(Return());

  EXPECT_CALL(*mock_display_coordinator_, CommitConfig(_, _)).Times(1).WillOnce(Return());

  // RenderFrame with empty render data list for the display.
  // This triggers SetRenderDataOnDisplay with 0 images.
  RenderData render_data = {.display_id = kDisplayId, .layers = {}};
  std::span<const RenderData> render_data_list(&render_data, 1);
  display_compositor_->RenderFrame(1, zx::time_monotonic(1), render_data_list, {}, {}, {},
                                   [](const scheduling::Timestamps&) {});

  // Cleanup: All layers should be destroyed.
  for (uint64_t i = 1; i < layer_id_value; ++i) {
    EXPECT_CALL(
        *mock_display_coordinator_,
        DestroyLayer(
            MatchRequestField(DestroyLayer, layer_id, Eq(display::WireLayerId{.value = i})), _))
        .Times(1)
        .WillOnce(Return());
  }
}

TEST_F(DisplayCompositorTest, TryDirectToDisplayExceedsHardwareLayerLimitFallbackToGpu) {
  static constexpr display::DisplayId kDisplayId(1);
  static constexpr glm::uvec2 resolution(1024, 768);
  static constexpr uint32_t kMaxDisplayLayersCount = 1;
  const DisplayInfo display_info = {resolution, {kPixelFormat}, kMaxDisplayLayersCount};

  // Note: Scenic creates display->max_layer_count() layers for the pool PLUS
  // one additional layer for the empty scene. Total = kMaxDisplayLayersCount + 1.
  EXPECT_CALL(*mock_display_coordinator_, CreateLayer(_, _))
      .Times(kMaxDisplayLayersCount + 1)
      .WillRepeatedly(
          testing::Invoke([&](auto, MockDisplayCoordinator::CreateLayerCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));

  display::Display display({kDisplayId.ToFidl()}, resolution.x, resolution.y,
                           kMaxDisplayLayersCount);
  display_compositor_->AddDisplay(&display, display_info, /* num_vmos= */ 0,
                                  /* out_buffer_collection= */ nullptr);

  auto root_image_id = allocation::GenerateUniqueImageId();
  auto child_image_id = allocation::GenerateUniqueImageId();

  ResolvedLayer layer1 = {
      .content = ResolvedLayer::ImageContent{.image_id = root_image_id},
  };
  ResolvedLayer layer2 = {
      .content = ResolvedLayer::ImageContent{.image_id = child_image_id},
  };
  std::array layers = {layer1, layer2};
  RenderData render_data = {.display_id = kDisplayId, .layers = layers};
  std::span<const RenderData> render_data_list(&render_data, 1);

  EXPECT_CALL(*mock_display_coordinator_, SetDisplayLayers(_, _)).Times(0);

  ASSERT_EQ(render_data_list[0].layers.size(), 2u);
  bool result = TryDirectToDisplay(render_data_list);
  EXPECT_FALSE(result);

  // Cleanup: All layers (kMaxDisplayLayersCount + 1 empty scene layer) should be destroyed.
  for (uint64_t i = 1; i <= kMaxDisplayLayersCount + 1; ++i) {
    EXPECT_CALL(
        *mock_display_coordinator_,
        DestroyLayer(
            MatchRequestField(DestroyLayer, layer_id, Eq(display::WireLayerId{.value = i})), _))
        .Times(1)
        .WillOnce(Return());
  }
  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());
}

TEST_F(DisplayCompositorTest, SolidColorContentTakesColorLayerPath) {
  const display::DisplayId kDisplayId(1);
  glm::uvec2 resolution(1024, 768);
  DisplayInfo display_info = {resolution, {kPixelFormat}, kMaxDisplayLayersCount};
  display::Display display({kDisplayId.ToFidl()}, resolution.x, resolution.y,
                           kMaxDisplayLayersCount);

  uint64_t layer_id_value = 1;
  EXPECT_CALL(*mock_display_coordinator_, CreateLayer(_, _))
      .Times(kMaxDisplayLayersCount + 1)
      .WillRepeatedly(testing::Invoke(
          [&](fidl::WireServer<fuchsia_hardware_display::Coordinator>::CreateLayerRequestView
                  request,
              MockDisplayCoordinator::CreateLayerCompleter::Sync& completer) {
            EXPECT_EQ(request->layer_id.value, layer_id_value++);
            completer.Reply(fit::ok());
          }));

  display_compositor_->AddDisplay(&display, display_info, /*num_vmos*/ 0,
                                  /*out_buffer_collection*/ nullptr);

  ResolvedLayer layer = {
      .geometry = SrcToDest(types::RectangleF({.x = 0,
                                               .y = 0,
                                               .width = static_cast<float>(resolution.x),
                                               .height = static_cast<float>(resolution.y)})),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content = ResolvedLayer::SolidColorContent{.color = {1.f, 0.f, 0.f, 1.f}},
  };
  RenderData render_data = {
      .display_id = kDisplayId,
      .layers = std::span<const ResolvedLayer>(&layer, 1),
  };

  EXPECT_CALL(*mock_display_coordinator_, SetLayerColorConfig(_, _))
      .Times(2)
      .WillRepeatedly(Return());
  EXPECT_CALL(*mock_display_coordinator_, SetLayerPrimaryConfig(_, _)).Times(0);
  EXPECT_CALL(*mock_display_coordinator_, SetLayerImage2(_, _)).Times(0);

  EXPECT_CALL(*mock_display_coordinator_, SetDisplayLayers(_, _)).Times(1).WillOnce(Return());
  EXPECT_CALL(*mock_display_coordinator_, SetDisplayMode(_, _)).Times(1).WillOnce(Return());
  EXPECT_CALL(*mock_display_coordinator_, CheckConfig(_))
      .Times(1)
      .WillOnce(testing::Invoke([](MockDisplayCoordinator::CheckConfigCompleter::Sync& completer) {
        completer.Reply(display::WireConfigResult::kOk);
      }));
  EXPECT_CALL(*mock_display_coordinator_, CommitConfig(_, _)).Times(1).WillOnce(Return());

  bool result = TryDirectToDisplay(std::span<const RenderData>(&render_data, 1));
  EXPECT_TRUE(result);

  // Cleanup: All layers should be destroyed.
  for (uint64_t i = 1; i <= kMaxDisplayLayersCount + 1; ++i) {
    EXPECT_CALL(
        *mock_display_coordinator_,
        DestroyLayer(
            MatchRequestField(DestroyLayer, layer_id, Eq(display::WireLayerId{.value = i})), _))
        .Times(1)
        .WillOnce(Return());
  }
  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());
}

TEST_F(DisplayCompositorTest, ImageContentTakesImageLayerPath) {
  const uint64_t kGlobalBufferCollectionId = allocation::GenerateUniqueBufferCollectionId();
  const display::WireBufferCollectionId kDisplayBufferCollectionId =
      display::ToDisplayFidlBufferCollectionId(kGlobalBufferCollectionId);

  const display::DisplayId kDisplayId(1);
  glm::uvec2 resolution(1024, 768);
  DisplayInfo display_info = {resolution, {kPixelFormat}, kMaxDisplayLayersCount};
  display::Display display({kDisplayId.ToFidl()}, resolution.x, resolution.y,
                           kMaxDisplayLayersCount);

  uint64_t layer_id_value = 1;
  EXPECT_CALL(*mock_display_coordinator_, CreateLayer(_, _))
      .Times(kMaxDisplayLayersCount + 1)
      .WillRepeatedly(testing::Invoke(
          [&](fidl::WireServer<fuchsia_hardware_display::Coordinator>::CreateLayerRequestView
                  request,
              MockDisplayCoordinator::CreateLayerCompleter::Sync& completer) {
            EXPECT_EQ(request->layer_id.value, layer_id_value++);
            completer.Reply(fit::ok());
          }));

  display_compositor_->AddDisplay(&display, display_info, /*num_vmos*/ 0,
                                  /*out_buffer_collection*/ nullptr);

  // Import buffer collection and image.
  EXPECT_CALL(*mock_display_coordinator_,
              ImportBufferCollection(MatchRequestField(ImportBufferCollection, buffer_collection_id,
                                                       Eq(kDisplayBufferCollectionId)),
                                     _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorImportBufferCollectionRequest* request,
             MockDisplayCoordinator::ImportBufferCollectionCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));
  EXPECT_CALL(*mock_display_coordinator_,
              SetBufferCollectionConstraints(
                  MatchRequestField(SetBufferCollectionConstraints, buffer_collection_id,
                                    Eq(kDisplayBufferCollectionId)),
                  _))
      .Times(1)
      .WillOnce(testing::Invoke(
          [](fuchsia_hardware_display::wire::CoordinatorSetBufferCollectionConstraintsRequest*,
             MockDisplayCoordinator::SetBufferCollectionConstraintsCompleter::Sync& completer) {
            completer.Reply(fit::ok());
          }));

  // Save token to avoid early token failure.
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token_ref;
  EXPECT_CALL(*renderer_, ImportBufferCollection(kGlobalBufferCollectionId, _, _, _, _))
      .WillOnce([&token_ref](auto id, auto& client, auto token, auto usage, auto size) {
        token_ref = std::move(token);
        return fpromise::make_ok_promise();
      });
  EXPECT_TRUE(RunPromise(display_compositor_->ImportBufferCollection(
      kGlobalBufferCollectionId, sysmem_allocator_, CreateToken(),
      BufferCollectionUsage::kClientImage, std::nullopt)));
  SetDisplaySupported(kGlobalBufferCollectionId, true);

  ImageMetadata image_metadata = ImageMetadata{
      .collection_id = kGlobalBufferCollectionId,
      .identifier = allocation::GenerateUniqueImageId(),
      .vmo_index = 0,
      .width = 128,
      .height = 256,
  };

  const display::WireImageId fidl_image_id = image_metadata.identifier.ToFidl();
  EXPECT_CALL(
      *mock_display_coordinator_,
      ImportImage(testing::AllOf(MatchRequestField(ImportImage, buffer_collection_id,
                                                   Eq(kDisplayBufferCollectionId)),
                                 MatchRequestField(ImportImage, buffer_index, Eq(0)),
                                 MatchRequestField(ImportImage, image_id, Eq(fidl_image_id))),
                  _))
      .Times(1)
      .WillOnce(testing::Invoke([](fuchsia_hardware_display::wire::CoordinatorImportImageRequest*,
                                   MockDisplayCoordinator::ImportImageCompleter::Sync& completer) {
        completer.Reply(fit::ok());
      }));
  EXPECT_CALL(*renderer_, ImportBufferImage(image_metadata, _))
      .WillOnce(ReturnPromise(fpromise::ok()));

  EXPECT_TRUE(RunPromise(
      display_compositor_->ImportBufferImage(image_metadata, BufferCollectionUsage::kClientImage)));

  ResolvedLayer layer = {
      .geometry = SrcToDest(types::RectangleF({.x = 0, .y = 0, .width = 128.f, .height = 256.f}),
                            types::RectangleF({.x = 0, .y = 0, .width = 128.f, .height = 256.f}),
                            types::RotateFlip::kIdentity()),
      .multiply_color = {1.f, 1.f, 1.f, 1.f},
      .blend_mode = BlendMode::kReplace(),
      .content =
          ResolvedLayer::ImageContent{
              .image_id = image_metadata.identifier,
              .width = image_metadata.width,
              .height = image_metadata.height,
          },
  };
  RenderData render_data = {
      .display_id = kDisplayId,
      .layers = std::span<const ResolvedLayer>(&layer, 1),
  };

  EXPECT_CALL(*mock_display_coordinator_, SetLayerColorConfig(_, _)).Times(1).WillOnce(Return());
  EXPECT_CALL(*mock_display_coordinator_, SetLayerPrimaryConfig(_, _)).Times(1).WillOnce(Return());
  EXPECT_CALL(*mock_display_coordinator_, SetLayerPrimaryPosition(_, _))
      .Times(1)
      .WillOnce(Return());
  EXPECT_CALL(*mock_display_coordinator_, SetLayerPrimaryAlpha(_, _)).Times(1).WillOnce(Return());
  EXPECT_CALL(*mock_display_coordinator_, SetLayerImage2(_, _)).Times(1).WillOnce(Return());

  EXPECT_CALL(*mock_display_coordinator_, SetDisplayLayers(_, _)).Times(1).WillOnce(Return());
  EXPECT_CALL(*mock_display_coordinator_, SetDisplayMode(_, _)).Times(1).WillOnce(Return());
  EXPECT_CALL(*mock_display_coordinator_, CheckConfig(_))
      .Times(1)
      .WillOnce(testing::Invoke([](MockDisplayCoordinator::CheckConfigCompleter::Sync& completer) {
        completer.Reply(display::WireConfigResult::kOk);
      }));
  EXPECT_CALL(*mock_display_coordinator_, CommitConfig(_, _)).Times(1).WillOnce(Return());

  bool result = TryDirectToDisplay(std::span<const RenderData>(&render_data, 1));
  EXPECT_TRUE(result);

  // Cleanup: All layers should be destroyed.
  for (uint64_t i = 1; i <= kMaxDisplayLayersCount + 1; ++i) {
    EXPECT_CALL(
        *mock_display_coordinator_,
        DestroyLayer(
            MatchRequestField(DestroyLayer, layer_id, Eq(display::WireLayerId{.value = i})), _))
        .Times(1)
        .WillOnce(Return());
  }
  EXPECT_CALL(*mock_display_coordinator_, DiscardConfig(_)).Times(1).WillOnce(Return());
}

}  // namespace flatland::test
